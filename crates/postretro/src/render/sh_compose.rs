// SH compose compute pass: merges static base SH bands with animated per-light
// deltas into the "total" SH 3D textures consumed by all SH samplers.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::delta_sh_volumes::{DeltaShVolumesSection, PROBE_F16_COUNT};

use super::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, AnimatedLightBuffers, SH_BAND_COUNT, ShVolumeResources,
};

/// Bytes of per-light metadata uploaded to the GPU. Must match the WGSL
/// `DeltaLightMeta` struct in `sh_compose.wgsl`. std430 layout:
///
/// ```text
///   0..12  aabb_origin       vec3<f32>
///   12..16 cell_size         f32
///   16..28 grid_dimensions   vec3<u32>
///   28..32 probe_offset      u32  (offset into delta_probes, in f32s)
///   32..36 descriptor_index  u32  (index into descriptors[])
///   36..40 _pad0
///   40..44 _pad1
///   44..48 _pad2
/// ```
const DELTA_LIGHT_META_SIZE: usize = 48;

// f16→f32 expansion keeps probe storage at 27 f32s per probe (9 bands × RGB).
const PROBE_F32_COUNT: usize = PROBE_F16_COUNT;

/// GPU-side compose pass. Always present — levels without an SH section get
/// 1×1×1 dummy textures and a single workgroup dispatch. Unconditional
/// dispatch avoids branching in the frame loop.
pub struct ShComposeResources {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    /// Probe grid dimensions. Drives the dispatch shape — one thread per
    /// probe, rounded up to the (4,4,4) workgroup size.
    grid_dimensions: [u32; 3],
}

impl ShComposeResources {
    /// Build the compose pipeline and bind group. When `delta` is `None` or
    /// empty, the per-light loop bound is 0 and the result is a pure
    /// base→total copy.
    pub fn new(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        sh_section: Option<&postretro_level_format::sh_volume::ShVolumeSection>,
        delta: Option<&DeltaShVolumesSection>,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // Build per-light meta + flat probe buffer.
        // Probes are stored as f32 (expanded from f16) to avoid unpack
        // instructions in the shader. Per-light `probe_offset` is an f32 index.

        let (light_count, meta_bytes, probe_data) = build_delta_buffers(sh_section, delta);

        // wgpu rejects zero-sized storage buffers; pad to one slot so the
        // bind group is always valid. Shader gates on `delta_light_count`.
        let safe_meta_bytes: Vec<u8> = if meta_bytes.is_empty() {
            vec![0u8; DELTA_LIGHT_META_SIZE]
        } else {
            meta_bytes
        };
        let safe_probe_f32: Vec<f32> = if probe_data.is_empty() {
            vec![0.0; PROBE_F32_COUNT]
        } else {
            probe_data
        };

        let probe_bytes: Vec<u8> = safe_probe_f32
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();

        use wgpu::util::DeviceExt;
        let meta_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Delta Light Meta"),
            contents: &safe_meta_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Delta Probes"),
            contents: &probe_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Grid-dims uniform: vec3<u32> grid_dims, u32 delta_light_count.
        let mut grid_bytes = [0u8; 16];
        grid_bytes[0..4].copy_from_slice(&sh.grid_dimensions[0].to_ne_bytes());
        grid_bytes[4..8].copy_from_slice(&sh.grid_dimensions[1].to_ne_bytes());
        grid_bytes[8..12].copy_from_slice(&sh.grid_dimensions[2].to_ne_bytes());
        grid_bytes[12..16].copy_from_slice(&light_count.to_ne_bytes());
        let grid_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Grid Dims"),
            contents: &grid_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // Grid origin uniform: vec3<f32> grid_origin, f32 _pad, vec3<f32> cell_size, f32 _pad.
        // Used in the shader to convert probe indices to world-space positions.
        let (grid_origin, cell_size) = match sh_section {
            Some(s) => (s.grid_origin, s.cell_size),
            None => ([0.0; 3], [1.0; 3]),
        };
        let mut origin_bytes = [0u8; 32];
        origin_bytes[0..4].copy_from_slice(&grid_origin[0].to_ne_bytes());
        origin_bytes[4..8].copy_from_slice(&grid_origin[1].to_ne_bytes());
        origin_bytes[8..12].copy_from_slice(&grid_origin[2].to_ne_bytes());
        origin_bytes[16..20].copy_from_slice(&cell_size[0].to_ne_bytes());
        origin_bytes[20..24].copy_from_slice(&cell_size[1].to_ne_bytes());
        origin_bytes[24..28].copy_from_slice(&cell_size[2].to_ne_bytes());
        let origin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Grid Origin"),
            contents: &origin_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // Build the bind group layout + pipeline.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH Compose BGL"),
            entries: &compose_bgl_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SH Compose Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // curve_eval.wgsl provides `sample_curve_catmull_rom` used by the shader.
        let shader_source = concat!(
            include_str!("../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SH Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SH Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        debug_assert_eq!(sh.base_band_views.len(), SH_BAND_COUNT);
        debug_assert_eq!(sh.total_band_storage_views.len(), SH_BAND_COUNT);

        let mut entries: Vec<wgpu::BindGroupEntry> = Vec::with_capacity(SH_BAND_COUNT * 2 + 6);
        // Bindings 0..9: base SH band textures (sampled).
        for (i, view) in sh.base_band_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        // Bindings 9..18: total SH band textures (storage write).
        for (i, view) in sh.total_band_storage_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: (SH_BAND_COUNT + i) as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 18,
            resource: grid_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 19,
            resource: origin_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 20,
            resource: meta_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 21,
            resource: probe_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 22,
            resource: sh.animation.descriptors.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 23,
            resource: sh.animation.anim_samples.as_entire_binding(),
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Compose Bind Group"),
            layout: &bind_group_layout,
            entries: &entries,
        });

        // Keep the `AnimatedLightBuffers` import live; the type is borrowed
        // via bind group entries above, not held directly.
        let _ = std::marker::PhantomData::<AnimatedLightBuffers>;
        let _ = ANIMATION_DESCRIPTOR_SIZE;

        log::info!(
            "[Renderer] SH compose: base grid {}×{}×{}, {} animated delta light(s)",
            sh.grid_dimensions[0],
            sh.grid_dimensions[1],
            sh.grid_dimensions[2],
            light_count,
        );

        Self {
            pipeline,
            bind_group,
            grid_dimensions: sh.grid_dimensions,
        }
    }

    /// Encode the per-frame compose dispatch.
    pub fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SH Compose"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, uniform_bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        let wg_x = self.grid_dimensions[0].div_ceil(4).max(1);
        let wg_y = self.grid_dimensions[1].div_ceil(4).max(1);
        let wg_z = self.grid_dimensions[2].div_ceil(4).max(1);
        pass.dispatch_workgroups(wg_x, wg_y, wg_z);
    }
}

/// Build per-light meta bytes and flat f32 probe buffer for the GPU.
/// Returns `(animated_light_count, meta_bytes, probe_data)`.
/// Returns `(0, [], [])` when the section is absent or empty.
///
/// Each light's `descriptor_index` indexes into the SH section's
/// `animation_descriptors`, matching the order produced by the compiler.
fn build_delta_buffers(
    sh_section: Option<&postretro_level_format::sh_volume::ShVolumeSection>,
    delta: Option<&DeltaShVolumesSection>,
) -> (u32, Vec<u8>, Vec<f32>) {
    let Some(delta) = delta else {
        return (0, Vec::new(), Vec::new());
    };
    if delta.grids.is_empty() {
        return (0, Vec::new(), Vec::new());
    }

    // Out-of-range descriptor indices would silently read garbage in the shader.
    // Warn and flag with u32::MAX rather than failing the load.
    let descriptor_count = sh_section
        .map(|s| s.animation_descriptors.len() as u32)
        .unwrap_or(0);

    let mut meta_bytes: Vec<u8> = Vec::with_capacity(delta.grids.len() * DELTA_LIGHT_META_SIZE);
    let mut probe_data: Vec<f32> = Vec::new();

    for (i, grid) in delta.grids.iter().enumerate() {
        let probe_offset = probe_data.len() as u32;
        let descriptor_index = delta
            .header
            .animation_descriptor_indices
            .get(i)
            .copied()
            .unwrap_or(u32::MAX);
        if descriptor_index >= descriptor_count {
            log::warn!(
                "[Renderer] DeltaShVolumes light {} references descriptor index {} \
                 but SH section has only {} descriptors — light skipped (zero contribution)",
                i,
                descriptor_index,
                descriptor_count,
            );
            // Emit a meta record to keep per-light index alignment intact;
            // descriptor_index = u32::MAX lets the shader early-out.
        }

        // Write 48-byte per-light meta record.
        let start = meta_bytes.len();
        meta_bytes.resize(start + DELTA_LIGHT_META_SIZE, 0);
        let s = &mut meta_bytes[start..start + DELTA_LIGHT_META_SIZE];
        s[0..4].copy_from_slice(&grid.aabb_origin[0].to_ne_bytes());
        s[4..8].copy_from_slice(&grid.aabb_origin[1].to_ne_bytes());
        s[8..12].copy_from_slice(&grid.aabb_origin[2].to_ne_bytes());
        s[12..16].copy_from_slice(&grid.cell_size.to_ne_bytes());
        s[16..20].copy_from_slice(&grid.grid_dimensions[0].to_ne_bytes());
        s[20..24].copy_from_slice(&grid.grid_dimensions[1].to_ne_bytes());
        s[24..28].copy_from_slice(&grid.grid_dimensions[2].to_ne_bytes());
        s[28..32].copy_from_slice(&probe_offset.to_ne_bytes());
        s[32..36].copy_from_slice(&descriptor_index.to_ne_bytes());
        // s[36..48] = padding, already zero.

        // Expand f16→f32 to avoid `unpack2x16float` in the shader.
        // Typical scenes: ~1MB f16 → ~2MB f32, well within storage-buffer limits.
        for probe in &grid.probes {
            for &half in &probe.sh_coefficients_f16 {
                probe_data.push(f16_bits_to_f32(half));
            }
        }
    }

    (delta.grids.len() as u32, meta_bytes, probe_data)
}

/// IEEE 754 binary16 → f32. Subnormals supported; NaN preserved.
/// Inverse of `f32_to_f16_bits` in `sh_volume.rs`.
fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mant = (bits & 0x3ff) as u32;

    let f32_bits: u32 = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // Subnormal: normalize.
            let mut m = mant;
            let mut e: i32 = -14;
            while (m & 0x400) == 0 {
                m <<= 1;
                e -= 1;
            }
            let m = m & 0x3ff;
            let e_f32 = (e + 127) as u32;
            (sign << 31) | (e_f32 << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        // Inf or NaN.
        let m = mant << 13;
        (sign << 31) | (0xff << 23) | m
    } else {
        let e_f32 = exp + (127 - 15);
        (sign << 31) | (e_f32 << 23) | (mant << 13)
    };

    f32::from_bits(f32_bits)
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = Vec::with_capacity(SH_BAND_COUNT * 2 + 6);
    // Bindings 0..9: base SH band textures (sampled via textureLoad — no filtering needed).
    for i in 0..SH_BAND_COUNT {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        });
    }
    // Bindings 9..18: total SH band textures (storage write).
    for i in 0..SH_BAND_COUNT {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: (SH_BAND_COUNT + i) as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        });
    }
    // Binding 18: grid-dimensions + delta_light_count.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 18,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Binding 19: grid_origin + cell_size.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 19,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Binding 20: per-light delta meta.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 20,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Binding 21: flat delta probe data.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 21,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Bindings 22–23: animation descriptors and samples (shared with SH bind group).
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 22,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 23,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_compose_shader_parses_and_exports_compose_main() {
        // curve_eval.wgsl must be appended to resolve Catmull-Rom helpers.
        let src = concat!(
            include_str!("../shaders/sh_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module =
            naga::front::wgsl::parse_str(src).expect("sh_compose.wgsl should parse as WGSL");
        let has_compose = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "compose_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_compose, "compose_main entry point missing");
    }

    #[test]
    fn f16_bits_round_trip_for_simple_values() {
        use crate::render::sh_volume::f32_to_f16_bits;
        for v in [0.0f32, 1.0, -1.0, 0.5, 2.0, -0.25, 100.0] {
            let bits = f32_to_f16_bits(v);
            let back = f16_bits_to_f32(bits);
            assert!(
                (back - v).abs() < 1e-3,
                "round-trip failed for {v}: f16=0x{bits:04x}, back={back}",
            );
        }
    }

    #[test]
    fn build_delta_buffers_no_section_returns_empty() {
        let (count, meta, probes) = build_delta_buffers(None, None);
        assert_eq!(count, 0);
        assert!(meta.is_empty());
        assert!(probes.is_empty());
    }

    #[test]
    fn build_delta_buffers_empty_grids_returns_empty() {
        use postretro_level_format::delta_sh_volumes::{
            DeltaShVolumeHeader, DeltaShVolumesSection,
        };
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: Vec::new(),
            },
            grids: Vec::new(),
        };
        let (count, meta, probes) = build_delta_buffers(None, Some(&section));
        assert_eq!(count, 0);
        assert!(meta.is_empty());
        assert!(probes.is_empty());
    }

    #[test]
    fn build_delta_buffers_packs_meta_and_probes() {
        use postretro_level_format::delta_sh_volumes::{
            DeltaLightGrid, DeltaShProbe, DeltaShVolumeHeader, DeltaShVolumesSection,
        };
        use postretro_level_format::sh_volume::{
            AnimationDescriptor, PROBE_STRIDE, ShVolumeSection,
        };

        // Two descriptors so indices {0, 1} pass validation.
        let sh = ShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [1, 1, 1],
            probe_stride: PROBE_STRIDE,
            probes: vec![Default::default()],
            animation_descriptors: vec![
                AnimationDescriptor {
                    period: 1.0,
                    phase: 0.0,
                    base_color: [1.0, 1.0, 1.0],
                    brightness: vec![1.0],
                    color: vec![],
                    direction: vec![],
                    start_active: 1,
                };
                2
            ],
        };
        let probe_a = DeltaShProbe {
            sh_coefficients_f16: [0; PROBE_F16_COUNT],
        };
        let grid_a = DeltaLightGrid {
            aabb_origin: [1.0, 2.0, 3.0],
            cell_size: 0.5,
            grid_dimensions: [2, 1, 1],
            probes: vec![probe_a; 2],
        };
        let grid_b = DeltaLightGrid {
            aabb_origin: [-1.0, 0.0, 0.0],
            cell_size: 1.0,
            grid_dimensions: [1, 1, 1],
            probes: vec![probe_a],
        };
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: vec![0, 1],
            },
            grids: vec![grid_a, grid_b],
        };

        let (count, meta, probes) = build_delta_buffers(Some(&sh), Some(&section));
        assert_eq!(count, 2);
        assert_eq!(meta.len(), 2 * DELTA_LIGHT_META_SIZE);
        // 2 probes × 27 + 1 probe × 27 = 81 f32s.
        assert_eq!(probes.len(), 81);

        // Light 0: probe_offset = 0, descriptor_index = 0.
        let p0 = u32::from_ne_bytes(meta[28..32].try_into().unwrap());
        let d0 = u32::from_ne_bytes(meta[32..36].try_into().unwrap());
        assert_eq!(p0, 0);
        assert_eq!(d0, 0);

        // Light 1: probe_offset = 2 × 27 = 54, descriptor_index = 1.
        let p1 = u32::from_ne_bytes(
            meta[DELTA_LIGHT_META_SIZE + 28..DELTA_LIGHT_META_SIZE + 32]
                .try_into()
                .unwrap(),
        );
        let d1 = u32::from_ne_bytes(
            meta[DELTA_LIGHT_META_SIZE + 32..DELTA_LIGHT_META_SIZE + 36]
                .try_into()
                .unwrap(),
        );
        assert_eq!(p1, 54);
        assert_eq!(d1, 1);

        // Spot-check light 0 spatial fields.
        let ox = f32::from_ne_bytes(meta[0..4].try_into().unwrap());
        let cs = f32::from_ne_bytes(meta[12..16].try_into().unwrap());
        let dx = u32::from_ne_bytes(meta[16..20].try_into().unwrap());
        assert_eq!(ox, 1.0);
        assert_eq!(cs, 0.5);
        assert_eq!(dx, 2);
    }
}
