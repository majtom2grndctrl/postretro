// SH compose compute pass: merges static base SH bands with animated per-light
// deltas into the "total" SH 3D textures consumed by all SH samplers.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;

use super::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, AnimatedLightBuffers, SH_BAND_COUNT, ShVolumeResources,
};

// SH Compose Bind Group (`@group(1)`) binding index assignments. The shader
// mirrors these (changing either requires updating both).
//
//   @group(1):
//     0..9   base SH band textures        (sampled, `0..SH_BAND_COUNT`)
//     9..18  total SH band textures       (storage write, `SH_BAND_COUNT..2*SH_BAND_COUNT`)
//     18     GridDims uniform             (vec3<u32> grid_dims, f32 _pad)
//     19     GridOrigin uniform           (grid_origin + cell_size)
//     20     delta_subblocks  (storage)   f16 payload, raw `u16` halves; shader `unpack2x16float`s
//     21     affinity_offsets (storage)   `u32` CSR offsets (affinity_cell_count + 1)
//     22     animation descriptors        (storage, shared with the SH bind group)
//     23     animation samples            (storage, shared with the SH bind group)
//     24     affinity_lights  (storage)   `u32` flat light indices, CSR-parallel to delta subblocks
//
// 20/21 replace the old dense per-light `DeltaLightMeta`/`delta_probes` pair.
// 24 is numbered after the shared 22/23 so adding `affinity_lights` doesn't
// renumber the animation bindings shared with the SH bind group.

const BIND_DELTA_SUBBLOCKS: u32 = 20;
const BIND_AFFINITY_OFFSETS: u32 = 21;
const BIND_AFFINITY_LIGHTS: u32 = 24;

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
    /// empty, all CSR offset ranges are empty (`start == end`), so the result is
    /// a pure base→total copy.
    pub fn new(
        device: &wgpu::Device,
        sh: &ShVolumeResources,
        sh_section: Option<&postretro_level_format::sh_volume::ShVolumeSection>,
        delta: Option<&DeltaShVolumesSection>,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // Build the sparse CSR delta buffers. Probes stay f16 (raw `u16` halves)
        // in the storage buffer — the shader `unpack2x16float`s them. No
        // f16→f32 expansion.
        let buffers = build_delta_buffers(delta);
        let light_count = buffers.animated_light_count;

        // wgpu rejects zero-sized storage buffers; pad each to a minimum size so
        // the bind group is always valid. The shader's per-cell loop runs zero
        // times when `affinity_offsets[cell] == affinity_offsets[cell + 1]`, so
        // the padded `delta_subblocks`/`affinity_lights` contents are never read.
        //
        // `affinity_offsets` is the exception: the shader reads both
        // `affinity_offsets[cell]` and `affinity_offsets[cell + 1]` before
        // entering the loop, so the empty case must pad to two `u32`s (8 bytes).
        // Both are zero, so `start == end` and the loop skips — but `[0]` and
        // `[1]` are genuinely in bounds rather than relying on OOB clamping.
        let subblock_bytes = pad_storage_bytes(u16_slice_to_bytes(&buffers.delta_subblocks), 4);
        let offsets_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_offsets), 8);
        let lights_bytes = pad_storage_bytes(u32_slice_to_bytes(&buffers.affinity_lights), 4);

        use wgpu::util::DeviceExt;
        let delta_subblocks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Delta Subblocks (f16)"),
            contents: &subblock_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let affinity_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Affinity Offsets"),
                contents: &offsets_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let affinity_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH Compose Affinity Lights"),
            contents: &lights_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Footprint AC: report per-binding byte sizes of every `@group(1)`
        // storage buffer the compose pass binds, plus the combined total. The
        // CSR form should keep this well under the storage-buffer binding floor
        // regardless of animated-light count.
        let footprint = ComposeStorageFootprint {
            delta_subblocks_bytes: subblock_bytes.len(),
            affinity_offsets_bytes: offsets_bytes.len(),
            affinity_lights_bytes: lights_bytes.len(),
        };
        footprint.log();

        // Grid-dims uniform: vec3<u32> grid_dims, f32 _pad. The loop bound comes
        // from the affinity-cell CSR offsets; the 4th slot is now padding (the
        // `delta_scale` knob was retired with the indirect-only delta — the full
        // delta is always added). std140 needs the trailing word, so it stays.
        let mut grid_bytes = [0u8; 16];
        grid_bytes[0..4].copy_from_slice(&sh.grid_dimensions[0].to_ne_bytes());
        grid_bytes[4..8].copy_from_slice(&sh.grid_dimensions[1].to_ne_bytes());
        grid_bytes[8..12].copy_from_slice(&sh.grid_dimensions[2].to_ne_bytes());
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
            binding: BIND_DELTA_SUBBLOCKS,
            resource: delta_subblocks_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_AFFINITY_OFFSETS,
            resource: affinity_offsets_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_AFFINITY_LIGHTS,
            resource: affinity_lights_buffer.as_entire_binding(),
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

    /// Encode the per-frame compose dispatch. The accumulated animated delta is
    /// always added to the base at full weight (the `delta_scale` knob was
    /// retired with the indirect-only delta).
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

/// Per-binding byte sizes of the `@group(1)` storage buffers the compose pass
/// owns (the CSR delta payload + the two CSR index buffers). The sampled/storage
/// SH band textures and the two shared animation buffers are not counted here —
/// this is the footprint the sparse-delta plan exists to bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ComposeStorageFootprint {
    pub delta_subblocks_bytes: usize,
    pub affinity_offsets_bytes: usize,
    pub affinity_lights_bytes: usize,
}

impl ComposeStorageFootprint {
    pub fn total_bytes(&self) -> usize {
        self.delta_subblocks_bytes + self.affinity_offsets_bytes + self.affinity_lights_bytes
    }

    /// Emit the footprint AC log line. MiB to two decimals for readability.
    fn log(&self) {
        let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
        log::info!(
            "[Renderer] SH compose @group(1) storage footprint: \
             delta_subblocks {:.2} MiB ({} B), affinity_offsets {:.2} MiB ({} B), \
             affinity_lights {:.2} MiB ({} B) — total {:.2} MiB ({} B)",
            mib(self.delta_subblocks_bytes),
            self.delta_subblocks_bytes,
            mib(self.affinity_offsets_bytes),
            self.affinity_offsets_bytes,
            mib(self.affinity_lights_bytes),
            self.affinity_lights_bytes,
            mib(self.total_bytes()),
            self.total_bytes(),
        );
    }
}

/// CPU-side mirror of the sparse CSR delta buffers, ready to upload as GPU
/// storage buffers. `delta_subblocks` stays f16 (`u16` halves) — no expansion.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeltaComposeBuffers {
    pub animated_light_count: u32,
    /// f16 payload, one 64-probe stride-28 sub-block per CSR entry.
    pub delta_subblocks: Vec<u16>,
    /// CSR offsets, one per affinity cell plus a trailing total.
    pub affinity_offsets: Vec<u32>,
    /// Flat CSR light indices, index-parallel to the sub-blocks.
    pub affinity_lights: Vec<u32>,
}

/// Map the loaded `DeltaShVolumesSection` to the engine's compose buffers.
/// Pure (no GPU) so the loader→engine-struct mapping is unit-testable. When the
/// section is absent the buffers are empty and the shader does a base→total copy.
fn build_delta_buffers(delta: Option<&DeltaShVolumesSection>) -> DeltaComposeBuffers {
    let Some(delta) = delta else {
        return DeltaComposeBuffers {
            animated_light_count: 0,
            delta_subblocks: Vec::new(),
            affinity_offsets: Vec::new(),
            affinity_lights: Vec::new(),
        };
    };
    DeltaComposeBuffers {
        animated_light_count: delta.animation_descriptor_indices.len() as u32,
        // Keep f16 as raw halves — the shader unpacks them.
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
    }
}

fn u16_slice_to_bytes(data: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn u32_slice_to_bytes(data: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// wgpu rejects zero-sized storage buffer bindings. Pad an empty payload up to
/// `min_bytes` so the bind group stays valid for maps with no animated lights.
/// `min_bytes` is per-binding: `delta_subblocks`/`affinity_lights` need a single
/// element (their slots live inside the never-entered per-cell loop), while
/// `affinity_offsets` needs two `u32`s (8 bytes) because the shader reads both
/// `[cell]` and `[cell + 1]` before the loop bound is known.
fn pad_storage_bytes(mut bytes: Vec<u8>, min_bytes: usize) -> Vec<u8> {
    if bytes.is_empty() {
        bytes.resize(min_bytes, 0);
    }
    bytes
}

/// IEEE 754 binary16 → f32. Subnormals supported; NaN preserved.
/// Inverse of `f32_to_f16_bits` in `sh_volume.rs`. The compose pass no longer
/// expands f16→f32 (deltas stay f16 on the GPU), so the only non-test consumer
/// is the dev-tools SH probe-marker readback in `sh_diagnostics`.
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub(crate) fn f16_bits_to_f32(bits: u16) -> f32 {
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
    // Binding 18: grid-dimensions (+ trailing pad word; the delta_scale knob was retired).
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
    // Binding 20: delta_subblocks — sparse f16 probe payload (raw `u16` halves).
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_DELTA_SUBBLOCKS,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Binding 21: affinity_offsets — CSR offsets (`u32`).
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_AFFINITY_OFFSETS,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Binding 24: affinity_lights — flat CSR light indices (`u32`). Numbered
    // after the shared 22/23 so those keep their indices.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_AFFINITY_LIGHTS,
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

    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DeltaShVolumesSection, PROBE_F16_STRIDE, PROBES_PER_CELL,
    };

    /// One stride-28 sub-block (64 probes) of deterministic f16 halves.
    fn sample_subblock(seed: u16) -> Vec<u16> {
        (0..PROBES_PER_CELL * PROBE_F16_STRIDE)
            .map(|i| seed.wrapping_add(i as u16))
            .collect()
    }

    #[test]
    fn build_delta_buffers_no_section_returns_empty() {
        let b = build_delta_buffers(None);
        assert_eq!(b.animated_light_count, 0);
        assert!(b.delta_subblocks.is_empty());
        assert!(b.affinity_offsets.is_empty());
        assert!(b.affinity_lights.is_empty());
    }

    #[test]
    fn build_delta_buffers_maps_section_fields_keeping_f16() {
        // Three affinity cells, two animated lights; cell 0 → light 0, cell 2 →
        // light 1 (cell 1 empty). f16 halves must pass through unmodified — no
        // expansion to f32.
        let mut subblocks = sample_subblock(10);
        subblocks.extend(sample_subblock(200));
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            animation_descriptor_indices: vec![4, u32::MAX],
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks.clone(),
        };

        let b = build_delta_buffers(Some(&section));
        assert_eq!(b.animated_light_count, 2);
        assert_eq!(b.affinity_offsets, vec![0, 1, 1, 2]);
        assert_eq!(b.affinity_lights, vec![0, 1]);
        // f16 payload preserved bit-for-bit (still u16, not expanded).
        assert_eq!(b.delta_subblocks, subblocks);
        assert_eq!(
            b.delta_subblocks.len(),
            2 * PROBES_PER_CELL * PROBE_F16_STRIDE
        );
    }

    #[test]
    fn compose_footprint_byte_sizes_match_payloads() {
        // Two CSR entries → two sub-blocks; affinity_dims [3,1,1] → 4 offsets.
        let mut subblocks = sample_subblock(1);
        subblocks.extend(sample_subblock(2));
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            animation_descriptor_indices: vec![0, 1],
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks,
        };
        let b = build_delta_buffers(Some(&section));

        let footprint = ComposeStorageFootprint {
            delta_subblocks_bytes: u16_slice_to_bytes(&b.delta_subblocks).len(),
            affinity_offsets_bytes: u32_slice_to_bytes(&b.affinity_offsets).len(),
            affinity_lights_bytes: u32_slice_to_bytes(&b.affinity_lights).len(),
        };

        // 2 entries × 64 probes × 28 halves × 2 bytes.
        assert_eq!(
            footprint.delta_subblocks_bytes,
            2 * PROBES_PER_CELL * PROBE_F16_STRIDE * 2
        );
        // 4 offsets × 4 bytes, 2 lights × 4 bytes.
        assert_eq!(footprint.affinity_offsets_bytes, 4 * 4);
        assert_eq!(footprint.affinity_lights_bytes, 2 * 4);
        assert_eq!(
            footprint.total_bytes(),
            footprint.delta_subblocks_bytes + 16 + 8
        );
    }

    #[test]
    fn pad_storage_bytes_pads_empty_to_min() {
        assert_eq!(pad_storage_bytes(Vec::new(), 4), vec![0u8; 4]);
        // affinity_offsets pads to two u32s (8 bytes) so the shader's
        // `[cell]`/`[cell + 1]` reads are both in bounds (both zero → loop skips).
        assert_eq!(pad_storage_bytes(Vec::new(), 8), vec![0u8; 8]);
        // Non-empty payloads pass through unchanged regardless of min_bytes.
        assert_eq!(
            pad_storage_bytes(vec![1, 2, 3, 4, 5], 4),
            vec![1, 2, 3, 4, 5]
        );
    }
}
