// SDF static-occluder atlas GPU resources: 3D distance atlas texture +
// top-level index storage buffer + coarse-distance 3D texture + meta uniform.
// Owned by the renderer; bound only by the (Task 4) half-resolution shadow
// pass. NOT bound by the forward pass — the forward pass receives only the
// upsampled shadow-factor texture (Task 5, separate binding in group 5).
//
// Section absence (legacy PRL, empty bake) yields the "no atlas" state: the
// `present` flag is false and dummy 1×1×1 textures + minimum-size buffers are
// allocated so the bind group stays valid. The shadow pass guards on
// `present` and skips its dispatch entirely in that case.
//
// See: context/plans/in-progress/sdf-static-occluder-shadows/index.md (Task 3)

use postretro_level_format::sdf_atlas::SdfAtlasSection;
use wgpu::util::DeviceExt;

/// Width (in bytes) of one `SdfAtlasMeta` uniform record on the GPU.
/// Two vec4 slots for world_min+voxel_size and world_max+brick_size, plus
/// two uvec4 slots for grid_dims+surface_brick_count and
/// atlas_bricks_per_axis+present_flag. 64 bytes total — std140-aligned.
///
/// WGSL layout (must match the consumer shader once Task 4 lands):
///   0..12   world_min            (vec3<f32>)
///   12..16  voxel_size_m         (f32)
///   16..28  world_max            (vec3<f32>)
///   28..32  brick_size_voxels    (u32, reinterpreted as f32 if convenient)
///   32..44  grid_dims            (vec3<u32>)
///   44..48  surface_brick_count  (u32)
///   48..60  atlas_bricks_per_axis (vec3<u32>)
///   60..64  present              (u32, 0 = no atlas, 1 = present)
pub const SDF_ATLAS_META_SIZE: usize = 64;

/// On-GPU representation of the bake's `i16` quantization scale. Stored in
/// the meta uniform's `voxel_size_m` field — the bake quantizes distances at
/// `voxel_size_m / 256` per `i16` step, and the runtime tracer recovers
/// metric distance with that constant.
#[allow(dead_code)]
pub const SDF_I16_QUANT_STEPS_PER_VOXEL: f32 = 256.0;

/// Uploaded SDF atlas GPU resources + bind group.
///
/// Always populated — when the PRL has no SDF section (legacy maps,
/// degenerate empty-geometry bakes), `present` is `false` and dummy
/// minimum-size resources are bound so the bind group remains valid. The
/// shadow pass (Task 4) reads `present` to skip its dispatch.
///
/// Bind group layout (group index is the shadow pass's choice; this module
/// owns the layout — it is not added to forward bind groups):
///   binding 0: SdfAtlasMeta uniform
///   binding 1: 3D `R16Sint` atlas distances (sampled, COMPUTE visibility)
///   binding 2: 3D `R32Float` coarse per-brick distances (sampled, COMPUTE visibility)
///   binding 3: storage buffer of `u32` top-level slots (read-only, COMPUTE visibility)
pub struct SdfAtlasResources {
    /// Bind group consumed by the (future) Task 4 shadow pass.
    #[allow(dead_code)]
    pub bind_group: wgpu::BindGroup,
    /// Bind-group layout the Task 4 shadow pass references when building
    /// its pipeline layout.
    #[allow(dead_code)]
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// `true` when a non-empty SDF atlas section was uploaded. `false` →
    /// dummy 1×1×1 textures + 1-element top-level buffer; consumers skip
    /// the shadow pass.
    pub present: bool,
    /// Mirrors the section's `grid_dims` for the shadow pass to derive the
    /// world→brick transform without re-reading the section. Zero when
    /// `present == false`.
    #[allow(dead_code)]
    pub grid_dims: [u32; 3],
    /// Mirrors the section's `world_min`/`world_max`/`voxel_size_m`. Carried
    /// here so the shadow pass can compose world-space sample positions
    /// without sampling the meta uniform on the CPU.
    #[allow(dead_code)]
    pub world_min: [f32; 3],
    #[allow(dead_code)]
    pub world_max: [f32; 3],
    #[allow(dead_code)]
    pub voxel_size_m: f32,
    #[allow(dead_code)]
    pub brick_size_voxels: u32,
    /// Owned so the views above stay valid for the renderer's lifetime.
    #[allow(dead_code)]
    atlas_texture: wgpu::Texture,
    #[allow(dead_code)]
    coarse_texture: wgpu::Texture,
    #[allow(dead_code)]
    top_level_buffer: wgpu::Buffer,
    #[allow(dead_code)]
    meta_buffer: wgpu::Buffer,
}

impl SdfAtlasResources {
    /// Build SDF resources from an optional baked section.
    ///
    /// `None` (legacy PRL, missing section) and `Some(empty)` (zero grid
    /// dims) both yield the "no atlas" state: dummy 1×1×1 textures + a
    /// 1-element top-level buffer + a zeroed meta uniform with `present = 0`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&SdfAtlasSection>,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SDF Atlas Bind Group Layout"),
            entries: &Self::bind_group_layout_entries(),
        });

        // Filter out empty-geometry sections — the bake encodes "no SDF" as
        // zero grid dims, mirroring `ShVolumeSection`'s empty-volume
        // convention. Treat it the same as a missing section.
        let usable = section.filter(|s| {
            s.grid_dims[0] > 0 && s.grid_dims[1] > 0 && s.grid_dims[2] > 0
        });

        let present = usable.is_some();

        // Choose atlas/coarse/top-level shapes. For the absent case we still
        // need a valid wgpu binding, so allocate 1×1×1 dummies.
        let (
            grid_dims,
            world_min,
            world_max,
            voxel_size_m,
            brick_size_voxels,
            atlas_bricks_per_axis,
            surface_brick_count,
            atlas_size,
            atlas_payload,
            coarse_size,
            coarse_payload,
            top_level_payload,
        ) = if let Some(sec) = usable {
            let brick_size = sec.brick_size_voxels.max(1);
            // 3D atlas texture: `brick_size_voxels` along each brick axis
            // times the `atlas_bricks_per_axis` packing dimensions. The
            // bake's `surface_brick_count` and `atlas_bricks_per_axis`
            // together describe the 3D packing; the texture itself sizes to
            // `atlas_bricks_per_axis * brick_size` along each axis.
            let atlas_w = sec.atlas_bricks_per_axis[0].max(1) * brick_size;
            let atlas_h = sec.atlas_bricks_per_axis[1].max(1) * brick_size;
            let atlas_d = sec.atlas_bricks_per_axis[2].max(1) * brick_size;
            let expected_atlas_voxels = (atlas_w as usize)
                .saturating_mul(atlas_h as usize)
                .saturating_mul(atlas_d as usize);

            // If the bake's packed atlas is shorter than the texture extent
            // (the bake may pack only `surface_brick_count` bricks and leave
            // the trailing slots in the 3D layout empty), pad with zeros.
            // If it is longer, truncate — the texture can't hold more.
            let mut atlas_bytes = Vec::with_capacity(expected_atlas_voxels * 2);
            for v in sec.atlas.iter().take(expected_atlas_voxels) {
                atlas_bytes.extend_from_slice(&v.to_le_bytes());
            }
            atlas_bytes.resize(expected_atlas_voxels * 2, 0);

            // Coarse texture matches the brick-grid dims (one f32 per brick
            // cell). `coarse_distances.len()` should equal prod(grid_dims);
            // pad/truncate defensively to match the texture extent so a
            // malformed section still yields a valid upload.
            let expected_coarse = sec.total_bricks();
            let mut coarse_bytes = Vec::with_capacity(expected_coarse * 4);
            for v in sec.coarse_distances.iter().take(expected_coarse) {
                coarse_bytes.extend_from_slice(&v.to_le_bytes());
            }
            coarse_bytes.resize(expected_coarse * 4, 0);

            // Top-level: one u32 per brick cell. Same defensive pad/truncate
            // logic as the coarse texture.
            let mut top_level_bytes = Vec::with_capacity(expected_coarse * 4);
            for slot in sec.top_level.iter().take(expected_coarse) {
                top_level_bytes.extend_from_slice(&slot.to_le_bytes());
            }
            top_level_bytes.resize(expected_coarse * 4, 0);

            (
                sec.grid_dims,
                sec.world_min,
                sec.world_max,
                sec.voxel_size_m,
                brick_size,
                sec.atlas_bricks_per_axis,
                sec.surface_brick_count,
                wgpu::Extent3d {
                    width: atlas_w,
                    height: atlas_h,
                    depth_or_array_layers: atlas_d,
                },
                atlas_bytes,
                wgpu::Extent3d {
                    width: sec.grid_dims[0],
                    height: sec.grid_dims[1],
                    depth_or_array_layers: sec.grid_dims[2],
                },
                coarse_bytes,
                top_level_bytes,
            )
        } else {
            // Dummy 1×1×1 with `present = 0`. wgpu rejects zero-sized
            // bindings; the shadow pass guards on `present` before touching
            // contents, so the payloads can be any single zero element.
            (
                [0u32; 3],
                [0.0f32; 3],
                [0.0f32; 3],
                0.0f32,
                0u32,
                [0u32; 3],
                0u32,
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                vec![0u8; 2],
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                vec![0u8; 4],
                vec![0u8; 4],
            )
        };

        // 3D atlas distances: R16Sint matches the bake's `i16` quantization
        // exactly — no precision loss across the GPU boundary, and the
        // shadow pass scales by `voxel_size_m / 256` to recover meters.
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SDF Atlas Distances"),
            size: atlas_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R16Sint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas_payload,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(2 * atlas_size.width),
                rows_per_image: Some(atlas_size.height),
            },
            atlas_size,
        );

        // Coarse per-brick distances: R32Float in meters; one texel per
        // brick cell of the world grid.
        let coarse_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SDF Atlas Coarse Distances"),
            size: coarse_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &coarse_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &coarse_payload,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * coarse_size.width),
                rows_per_image: Some(coarse_size.height),
            },
            coarse_size,
        );

        // Top-level brick slot index buffer. `EMPTY`/`INTERIOR` sentinels
        // round-trip through unchanged; the shadow pass interprets them.
        let top_level_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SDF Atlas Top Level"),
            contents: &top_level_payload,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let meta_bytes = build_meta_bytes(
            world_min,
            world_max,
            voxel_size_m,
            brick_size_voxels,
            grid_dims,
            atlas_bricks_per_axis,
            surface_brick_count,
            present,
        );
        let meta_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SDF Atlas Meta Uniform"),
            contents: &meta_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SDF Atlas Distances View"),
            ..Default::default()
        });
        let coarse_view = coarse_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SDF Atlas Coarse View"),
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SDF Atlas Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: meta_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&coarse_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: top_level_buffer.as_entire_binding(),
                },
            ],
        });

        if present {
            log::info!(
                "[SdfAtlas] uploaded {}×{}×{} brick grid (voxel={:.4}m, brick={} voxels, {} surface bricks)",
                grid_dims[0],
                grid_dims[1],
                grid_dims[2],
                voxel_size_m,
                brick_size_voxels,
                surface_brick_count,
            );
        } else {
            log::info!(
                "[SdfAtlas] no atlas — shadow pass disabled (section absent or empty-geometry bake)"
            );
        }

        Self {
            bind_group,
            bind_group_layout,
            present,
            grid_dims,
            world_min,
            world_max,
            voxel_size_m,
            brick_size_voxels,
            atlas_texture,
            coarse_texture,
            top_level_buffer,
            meta_buffer,
        }
    }

    fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
        // COMPUTE-only visibility: only the shadow pass (Task 4) reads these.
        // The forward pass gets only the shadow-factor texture (Task 5), not
        // the atlas itself.
        let vis = wgpu::ShaderStages::COMPUTE;
        [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: vis,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Quantized i16 distances. `filterable: false` is honest — no
            // sampler is bound; the shadow pass reads via `textureLoad`
            // (signed integer formats are not filterable in wgpu anyway).
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: vis,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Sint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
            // Coarse f32 distances. Unfilterable here too — the shadow pass
            // uses `textureLoad` for both the atlas and the coarse fallback.
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: vis,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: vis,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ]
    }
}

/// Pack the SDF meta uniform. See the `SDF_ATLAS_META_SIZE` doc for the
/// 64-byte field layout. Kept as a free function so it can be unit-tested
/// without a wgpu device.
pub(crate) fn build_meta_bytes(
    world_min: [f32; 3],
    world_max: [f32; 3],
    voxel_size_m: f32,
    brick_size_voxels: u32,
    grid_dims: [u32; 3],
    atlas_bricks_per_axis: [u32; 3],
    surface_brick_count: u32,
    present: bool,
) -> [u8; SDF_ATLAS_META_SIZE] {
    let mut bytes = [0u8; SDF_ATLAS_META_SIZE];
    bytes[0..4].copy_from_slice(&world_min[0].to_le_bytes());
    bytes[4..8].copy_from_slice(&world_min[1].to_le_bytes());
    bytes[8..12].copy_from_slice(&world_min[2].to_le_bytes());
    bytes[12..16].copy_from_slice(&voxel_size_m.to_le_bytes());
    bytes[16..20].copy_from_slice(&world_max[0].to_le_bytes());
    bytes[20..24].copy_from_slice(&world_max[1].to_le_bytes());
    bytes[24..28].copy_from_slice(&world_max[2].to_le_bytes());
    bytes[28..32].copy_from_slice(&brick_size_voxels.to_le_bytes());
    bytes[32..36].copy_from_slice(&grid_dims[0].to_le_bytes());
    bytes[36..40].copy_from_slice(&grid_dims[1].to_le_bytes());
    bytes[40..44].copy_from_slice(&grid_dims[2].to_le_bytes());
    bytes[44..48].copy_from_slice(&surface_brick_count.to_le_bytes());
    bytes[48..52].copy_from_slice(&atlas_bricks_per_axis[0].to_le_bytes());
    bytes[52..56].copy_from_slice(&atlas_bricks_per_axis[1].to_le_bytes());
    bytes[56..60].copy_from_slice(&atlas_bricks_per_axis[2].to_le_bytes());
    let present_flag: u32 = if present { 1 } else { 0 };
    bytes[60..64].copy_from_slice(&present_flag.to_le_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_bytes_encode_world_bounds_and_present_flag() {
        let bytes = build_meta_bytes(
            [-8.0, -2.0, -8.0],
            [8.0, 6.0, 8.0],
            0.0625,
            4,
            [2, 3, 4],
            [2, 1, 1],
            5,
            true,
        );
        assert_eq!(bytes.len(), SDF_ATLAS_META_SIZE);

        let wmin_x = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let voxel = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let wmax_y = f32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let brick_size = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let grid_y = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
        let surface_count = u32::from_le_bytes(bytes[44..48].try_into().unwrap());
        let atlas_x = u32::from_le_bytes(bytes[48..52].try_into().unwrap());
        let present = u32::from_le_bytes(bytes[60..64].try_into().unwrap());

        assert_eq!(wmin_x, -8.0);
        assert_eq!(voxel, 0.0625);
        assert_eq!(wmax_y, 6.0);
        assert_eq!(brick_size, 4);
        assert_eq!(grid_y, 3);
        assert_eq!(surface_count, 5);
        assert_eq!(atlas_x, 2);
        assert_eq!(present, 1);
    }

    #[test]
    fn meta_bytes_present_flag_zero_when_absent() {
        let bytes = build_meta_bytes(
            [0.0; 3],
            [0.0; 3],
            0.0,
            0,
            [0; 3],
            [0; 3],
            0,
            false,
        );
        let present = u32::from_le_bytes(bytes[60..64].try_into().unwrap());
        assert_eq!(present, 0);
    }
}
