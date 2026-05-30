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
// See: context/plans/done/sdf-static-occluder-shadows/index.md (foundation)
//      context/plans/in-progress/sdf-filterable-atlas/index.md (current change)

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
///   binding 1: 3D `R16Float` atlas distances (filterable, COMPUTE visibility)
///   binding 2: 3D `R32Float` coarse per-brick distances (sampled, COMPUTE visibility)
///   binding 3: storage buffer of `u32` top-level slots (read-only, COMPUTE visibility)
///   binding 4: linear `Filtering` sampler for the fine atlas (COMPUTE visibility)
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
    #[allow(dead_code)]
    sampler: wgpu::Sampler,
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
        let usable =
            section.filter(|s| s.grid_dims[0] > 0 && s.grid_dims[1] > 0 && s.grid_dims[2] > 0);

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
            // Each surface brick is stored as a compact `(brick_size + 2)^3`
            // sub-cube (interior + 1-voxel apron per side, for hardware
            // trilinear). The 3D atlas texture sizes to
            // `atlas_bricks_per_axis * (brick_size + 2)` along each axis, and
            // each brick is scattered to a contiguous sub-cube at its tiled
            // 3D position so a single `textureSampleLevel` trilinear-filters it.
            let stored_edge = brick_size + 2;
            let atlas_w = sec.atlas_bricks_per_axis[0].max(1) * stored_edge;
            let atlas_h = sec.atlas_bricks_per_axis[1].max(1) * stored_edge;
            let atlas_d = sec.atlas_bricks_per_axis[2].max(1) * stored_edge;

            // Scatter the compact back-to-back surface bricks into a dense
            // f16 atlas at their tiled 3D positions. On-disk data stays i16;
            // the conversion to f16 (`R16Float`) happens here at upload.
            let atlas_f16 = scatter_bricks_to_atlas(
                &sec.atlas,
                sec.surface_brick_count,
                brick_size,
                sec.atlas_bricks_per_axis,
            );
            let mut atlas_bytes = Vec::with_capacity(atlas_f16.len() * 2);
            for bits in &atlas_f16 {
                atlas_bytes.extend_from_slice(&bits.to_le_bytes());
            }

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

        // 3D atlas distances: R16Float is filterable by default on every
        // backend (no feature flag), so one hardware `textureSampleLevel`
        // trilinear-filters the field. It carries the same quant-step
        // magnitudes the bake produced (decoded by `voxel_size_m / 256`);
        // f16 has ample precision for the small local distances stored here.
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SDF Atlas Distances"),
            size: atlas_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R16Float,
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

        // Linear sampler for the fine atlas — drives the hardware trilinear
        // `textureSampleLevel`. Clamp-to-edge so the half-texel margins at the
        // atlas extent don't wrap; the shader already keeps samples inside each
        // brick's apron'd sub-cube.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SDF Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
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
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
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
            sampler,
        }
    }

    fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
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
            // Fine f16 distances. `filterable: true` — the shadow pass samples
            // via `textureSampleLevel` with the linear sampler at binding 4 for
            // hardware trilinear (R16Float is filterable on all backends).
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: vis,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
            // Coarse f32 distances. Unfilterable — the shadow pass uses
            // `textureLoad` for the coarse fallback (no filtering needed at
            // brick granularity). The fine atlas at binding 1 is filterable and
            // is sampled via `textureSampleLevel` with the linear sampler at
            // binding 4.
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
            // Linear filtering sampler for the fine atlas trilinear sample.
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: vis,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    }
}

/// Pack the SDF meta uniform. See the `SDF_ATLAS_META_SIZE` doc for the
/// 64-byte field layout. Kept as a free function so it can be unit-tested
/// without a wgpu device.
#[allow(clippy::too_many_arguments)]
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

/// Scatter the bake's compact, back-to-back surface bricks into a dense f16
/// atlas buffer at their tiled 3D positions, converting i16 quant steps to f16
/// bits (`R16Float` write) along the way. `R16Float` is used rather than
/// keeping the on-disk `i16` as `R16Sint` because `R16Float` is filterable on
/// all wgpu backends without a feature flag, enabling the trilinear
/// `textureSampleLevel` in the shadow pass; `R16Sint` is not filterable.
///
/// Each surface brick is a contiguous `(brick_size + 2)^3` i16 block (interior
/// plus a 1-voxel apron on every side), z-major within the stored brick
/// (`vox_idx = sz·edge² + sy·edge + sx`, `edge = brick_size + 2`). Surface
/// brick `slot` maps to the 3D atlas-brick coord
/// `(slot % apx, (slot / apx) % apy, slot / (apx·apy))`; its texels start at
/// that coord `· (brick_size + 2)`. The output buffer is dense row-major over
/// the `(atlas_bricks_per_axis · (brick_size + 2))` extent — exactly what a
/// single `write_texture` expects. Kept device-free so the 3D scatter is
/// unit-testable per the data-logic / GPU-interaction split.
pub(crate) fn scatter_bricks_to_atlas(
    atlas_i16: &[i16],
    surface_brick_count: u32,
    brick_size: u32,
    atlas_bricks_per_axis: [u32; 3],
) -> Vec<u16> {
    let edge = (brick_size + 2) as usize;
    let voxels_per_brick = edge * edge * edge;
    let apx = atlas_bricks_per_axis[0].max(1) as usize;
    let apy = atlas_bricks_per_axis[1].max(1) as usize;
    let apz = atlas_bricks_per_axis[2].max(1) as usize;

    let atlas_w = apx * edge;
    let atlas_h = apy * edge;
    let atlas_d = apz * edge;
    let mut out = vec![0u16; atlas_w * atlas_h * atlas_d];

    let slots = (surface_brick_count as usize).min(atlas_i16.len() / voxels_per_brick.max(1));
    for slot in 0..slots {
        // Slot → 3D atlas-brick coord (z-major slot order — matches the shader).
        let bx = slot % apx;
        let by = (slot / apx) % apy;
        let bz = slot / (apx * apy);
        if bz >= apz {
            break; // packing overflow — shouldn't happen, but stay in bounds.
        }
        let base_x = bx * edge;
        let base_y = by * edge;
        let base_z = bz * edge;

        let brick = &atlas_i16[slot * voxels_per_brick..(slot + 1) * voxels_per_brick];
        // z-major within the stored brick: sz outermost, sx innermost.
        for sz in 0..edge {
            for sy in 0..edge {
                for sx in 0..edge {
                    let src = sz * edge * edge + sy * edge + sx;
                    let dst =
                        (base_z + sz) * atlas_w * atlas_h + (base_y + sy) * atlas_w + (base_x + sx);
                    out[dst] = super::sh_volume::f32_to_f16_bits(brick[src] as f32);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scatter_places_each_brick_as_contiguous_subcube() {
        // 2 surface bricks, brick_size = 1 → stored edge = 3, 27 voxels/brick.
        // Pack 2 bricks along x (apx = 2). Fill brick 0 with value v, brick 1
        // with value v+offset, then assert each lands at its tiled sub-cube and
        // the unwritten interior between/around is zero.
        let brick_size = 1u32;
        let edge = (brick_size + 2) as usize; // 3
        let vpb = edge * edge * edge; // 27
        let apx = 2usize;

        // Brick 0: stored voxel s gets value s (1..=26, with s=0 → 0 distinct
        // from the zero-initialized atlas). Brick 1: value s + 100.
        let mut atlas_i16 = vec![0i16; 2 * vpb];
        for s in 0..vpb {
            atlas_i16[s] = s as i16; // brick 0
            atlas_i16[vpb + s] = (s + 100) as i16; // brick 1
        }

        let out = scatter_bricks_to_atlas(&atlas_i16, 2, brick_size, [apx as u32, 1, 1]);
        let atlas_w = apx * edge; // 6
        let atlas_h = edge; // 3

        // Spot-check a few voxels of each brick map to the expected dense offset.
        for s in 0..vpb {
            let sx = s % edge;
            let sy = (s / edge) % edge;
            let sz = s / (edge * edge);

            // Brick 0 at atlas-brick (0,0,0): base offset 0.
            let dst0 = sz * atlas_w * atlas_h + sy * atlas_w + sx;
            assert_eq!(
                out[dst0],
                super::super::sh_volume::f32_to_f16_bits(s as f32)
            );

            // Brick 1 at atlas-brick (1,0,0): base x = edge.
            let dst1 = sz * atlas_w * atlas_h + sy * atlas_w + (edge + sx);
            assert_eq!(
                out[dst1],
                super::super::sh_volume::f32_to_f16_bits((s + 100) as f32)
            );
        }
    }

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
        let bytes = build_meta_bytes([0.0; 3], [0.0; 3], 0.0, 0, [0; 3], [0; 3], 0, false);
        let present = u32::from_le_bytes(bytes[60..64].try_into().unwrap());
        assert_eq!(present, 0);
    }
}
