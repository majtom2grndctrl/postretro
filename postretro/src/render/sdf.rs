// SDF atlas GPU resources: 3D texture, sampler, top-level index storage
// buffer, SdfMeta uniform, coarse per-brick distance 3D texture, and bind
// group (wired into group 2, bindings 5–9).
//
// See: context/plans/in-progress/lighting-foundation/8-sdf-shadows.md

use postretro_level_format::sdf_atlas::{BRICK_SLOT_EMPTY, SdfAtlasSection};

/// Binding offsets within group 2 for the SDF resources.
pub const BIND_SDF_ATLAS: u32 = 5;
pub const BIND_SDF_SAMPLER: u32 = 6;
pub const BIND_SDF_TOP_LEVEL: u32 = 7;
pub const BIND_SDF_META: u32 = 8;
pub const BIND_SDF_COARSE: u32 = 9;

/// Byte size of the `SdfMeta` uniform struct.
///
/// Layout (must match the WGSL `SdfMeta` struct in `forward.wgsl`):
///   0..12   world_min            (vec3<f32>)
///   12..16  voxel_size_m         (f32)
///   16..28  world_max            (vec3<f32>)
///   28..32  brick_size_voxels    (u32)
///   32..44  grid_dims            (vec3<u32>)
///   44..48  has_sdf_atlas        (u32, 0 or 1)
///   48..60  atlas_bricks         (vec3<u32>) — bricks-per-axis packed into atlas
///   60..64  _pad                 (u32)
pub const SDF_META_SIZE: usize = 64;

/// Maximum texture dimension the runtime is willing to request for the SDF
/// atlas 3D texture. Matches `wgpu::Limits::default().max_texture_dimension_3d`.
/// If the runtime ever requests a higher limit, this constant can follow.
pub const SDF_ATLAS_MAX_TEXTURE_DIM: u32 = 2048;

/// Compute how many bricks fit along each atlas axis, given a target surface
/// brick count, the per-brick voxel size, and the max-allowed texture
/// dimension (`axis_bricks * brick_size_voxels` must stay within this cap).
///
/// Returns `[ax, ay, az]`. Invariants:
/// - `ax * ay * az >= surface_count` when `surface_count > 0`
/// - Each `axis * brick_size_voxels <= max_texture_dim`
/// - `[1, 1, 1]` when `surface_count == 0` or `brick_size_voxels == 0`
///
/// Picks a cube-ish layout (`side = ceil(cbrt(n))`) so all three axes grow
/// together; a 1D Z-stack blows `max_texture_dimension_3d` on any real level.
pub fn atlas_brick_layout(
    surface_count: u32,
    brick_size_voxels: u32,
    max_texture_dim: u32,
) -> [u32; 3] {
    if surface_count == 0 || brick_size_voxels == 0 {
        return [1, 1, 1];
    }
    let max_per_axis = (max_texture_dim / brick_size_voxels).max(1) as u64;
    let n = surface_count as u64;
    let mut side = (n as f64).cbrt().ceil() as u64;
    if side < 1 {
        side = 1;
    }
    if side > max_per_axis {
        side = max_per_axis;
    }
    let ax = side;
    let ay = side;
    let layer = ax * ay;
    let az = n.div_ceil(layer);
    [ax as u32, ay as u32, az as u32]
}

/// Build the `SdfMeta` uniform bytes from section fields.
pub fn build_sdf_meta_bytes(
    world_min: [f32; 3],
    world_max: [f32; 3],
    voxel_size_m: f32,
    brick_size_voxels: u32,
    grid_dims: [u32; 3],
    has_sdf_atlas: bool,
    atlas_bricks: [u32; 3],
) -> [u8; SDF_META_SIZE] {
    let mut out = [0u8; SDF_META_SIZE];
    // world_min (vec3<f32>)
    out[0..4].copy_from_slice(&world_min[0].to_ne_bytes());
    out[4..8].copy_from_slice(&world_min[1].to_ne_bytes());
    out[8..12].copy_from_slice(&world_min[2].to_ne_bytes());
    // voxel_size_m (f32 in same slot)
    out[12..16].copy_from_slice(&voxel_size_m.to_ne_bytes());
    // world_max (vec3<f32>)
    out[16..20].copy_from_slice(&world_max[0].to_ne_bytes());
    out[20..24].copy_from_slice(&world_max[1].to_ne_bytes());
    out[24..28].copy_from_slice(&world_max[2].to_ne_bytes());
    // brick_size_voxels (u32 in same slot)
    out[28..32].copy_from_slice(&brick_size_voxels.to_ne_bytes());
    // grid_dims (vec3<u32>)
    out[32..36].copy_from_slice(&grid_dims[0].to_ne_bytes());
    out[36..40].copy_from_slice(&grid_dims[1].to_ne_bytes());
    out[40..44].copy_from_slice(&grid_dims[2].to_ne_bytes());
    // has_sdf_atlas (u32)
    let flag: u32 = if has_sdf_atlas { 1 } else { 0 };
    out[44..48].copy_from_slice(&flag.to_ne_bytes());
    // atlas_bricks (vec3<u32>)
    out[48..52].copy_from_slice(&atlas_bricks[0].to_ne_bytes());
    out[52..56].copy_from_slice(&atlas_bricks[1].to_ne_bytes());
    out[56..60].copy_from_slice(&atlas_bricks[2].to_ne_bytes());
    // _pad (u32) — already zero.
    out
}

/// Dequantize the atlas i16 values to f32 meters.
///
/// Quantization: 1 unit = voxel_size_m / 256.0 meters.
fn dequantize_atlas(atlas: &[i16], voxel_size_m: f32) -> Vec<f32> {
    let scale = voxel_size_m / 256.0;
    atlas.iter().map(|&v| v as f32 * scale).collect()
}

/// SDF atlas GPU resources. Always present; when no SDF section exists,
/// a dummy 1×1×1 texture and empty-ish buffers are bound and `has_sdf_atlas`
/// is 0 so the shader skips sphere-tracing.
pub struct SdfResources {
    /// The bound group entries — kept alive; the bind group holds wgpu refs
    /// but we must keep the underlying resources alive too.
    #[allow(dead_code)]
    atlas_texture: wgpu::Texture,
    #[allow(dead_code)]
    atlas_view: wgpu::TextureView,
    #[allow(dead_code)]
    sdf_sampler: wgpu::Sampler,
    #[allow(dead_code)]
    top_level_buffer: wgpu::Buffer,
    #[allow(dead_code)]
    meta_buffer: wgpu::Buffer,
    #[allow(dead_code)]
    coarse_texture: wgpu::Texture,
    #[allow(dead_code)]
    coarse_view: wgpu::TextureView,
}

impl SdfResources {
    /// Build the SDF atlas GPU resources. Returns `Self` with all GPU
    /// objects ready. Call `bind_group_entries` on the result to obtain the
    /// wgpu bind group entries (requires the entries to borrow `self`).
    pub fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&SdfAtlasSection>,
    ) -> Self {
        let usable = section.filter(|s| {
            s.brick_size_voxels > 0
                && s.grid_dims[0] > 0
                && s.grid_dims[1] > 0
                && s.grid_dims[2] > 0
                && !s.atlas.is_empty()
        });

        let sdf_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SDF Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let (atlas_texture, atlas_view, top_level_buffer, meta_buffer, coarse_texture, coarse_view) =
            if let Some(s) = usable {
                let brick_n = s.brick_size_voxels;
                let surface_count = s.surface_brick_count();

                // Cube-ish 3D packing: layout bricks into an atlas grid so no
                // texture axis exceeds `max_texture_dimension_3d`. A 1D Z-stack
                // (depth = brick_n * surface_count) overflows on any real
                // level — 32k surface bricks × 8 voxels blows the 2048 cap.
                let atlas_bricks_arr =
                    atlas_brick_layout(surface_count, brick_n, SDF_ATLAS_MAX_TEXTURE_DIM);
                let [ab_x, ab_y, ab_z] = atlas_bricks_arr;
                let tex_w = (ab_x * brick_n).max(1);
                let tex_h = (ab_y * brick_n).max(1);
                let tex_d = (ab_z * brick_n).max(1);

                // Dequantize i16 → f32 for the GPU texture.
                let f32_atlas = dequantize_atlas(&s.atlas, s.voxel_size_m);

                let extent = wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: tex_d,
                };

                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("SDF Atlas Texture 3D"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });

                // Upload. bytes_per_row must be a multiple of 256 for wgpu.
                let bytes_per_row = (tex_w * 4).next_multiple_of(256);
                let rows_per_layer = tex_h;

                // Build padded buffer covering the full atlas extent. Unused
                // brick slots stay zero — the top-level index never routes
                // real samples there, and half-texel clamping prevents
                // neighbor bleed into occupied bricks.
                let total_bytes =
                    (bytes_per_row as usize) * (rows_per_layer as usize) * (tex_d as usize);
                let mut padded = vec![0u8; total_bytes];
                let brick_n_usz = brick_n as usize;
                let bvol = brick_n_usz * brick_n_usz * brick_n_usz;
                for s_idx in 0..surface_count as usize {
                    let slot_base = s_idx * bvol;
                    let bxa = (s_idx as u32) % ab_x;
                    let bya = ((s_idx as u32) / ab_x) % ab_y;
                    let bza = (s_idx as u32) / (ab_x * ab_y);
                    for lz in 0..brick_n_usz {
                        for ly in 0..brick_n_usz {
                            let src_row = slot_base
                                + lz * brick_n_usz * brick_n_usz
                                + ly * brick_n_usz;
                            let tz = bza as usize * brick_n_usz + lz;
                            let ty = bya as usize * brick_n_usz + ly;
                            let dst_row_offset = tz * (bytes_per_row as usize) * (rows_per_layer as usize)
                                + ty * (bytes_per_row as usize);
                            let tx_base = bxa as usize * brick_n_usz;
                            for lx in 0..brick_n_usz {
                                let src_idx = src_row + lx;
                                let f = if src_idx < f32_atlas.len() {
                                    f32_atlas[src_idx]
                                } else {
                                    1.0
                                };
                                let dst = dst_row_offset + (tx_base + lx) * 4;
                                padded[dst..dst + 4].copy_from_slice(&f.to_ne_bytes());
                            }
                        }
                    }
                }

                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &padded,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(rows_per_layer),
                    },
                    extent,
                );

                let view = tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D3),
                    ..Default::default()
                });

                // Top-level index buffer: u32 per cell with sentinel remapping.
                // The atlas surface brick indices map to integer Z layers:
                //   slot → layer index in the 3D texture.
                // Sentinels (EMPTY = MAX, INTERIOR = MAX-1) are kept as-is;
                // the WGSL shader interprets them.
                let top_bytes: Vec<u8> = s
                    .top_level
                    .iter()
                    .flat_map(|&v| v.to_ne_bytes())
                    .collect();
                let top_buf = wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("SDF Top-Level Index Buffer"),
                        contents: &top_bytes,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    },
                );

                // Meta uniform.
                let meta_bytes = build_sdf_meta_bytes(
                    s.world_min,
                    s.world_max,
                    s.voxel_size_m,
                    s.brick_size_voxels,
                    s.grid_dims,
                    true,
                    atlas_bricks_arr,
                );
                let meta_buf = wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("SDF Meta Uniform"),
                        contents: &meta_bytes,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    },
                );

                log::info!(
                    "[SdfResources] Uploaded SDF atlas: brick={} tex_dims=[{}, {}, {}] \
                     atlas_bricks=[{}, {}, {}] surface_bricks={}",
                    brick_n,
                    tex_w,
                    tex_h,
                    tex_d,
                    ab_x,
                    ab_y,
                    ab_z,
                    surface_count,
                );

                // Coarse SDF texture: one texel per brick, trilinear-sampled
                // to give the sphere tracer valid distance data everywhere
                // in the grid (not just inside SURFACE bricks). R32Float, dims
                // exactly = grid_dims. See sdf_atlas::coarse_distances.
                let (coarse_tex, coarse_v) = upload_coarse_texture(
                    device,
                    queue,
                    s.grid_dims,
                    &s.coarse_distances,
                );

                (tex, view, top_buf, meta_buf, coarse_tex, coarse_v)
            } else {
                // Dummy resources: 1×1×1 texture, stub buffers.
                let extent = wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                };
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("SDF Atlas Dummy Texture 3D"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let dummy_f: f32 = 1.0;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &dummy_f.to_ne_bytes(),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256), // must be multiple of 256
                        rows_per_image: Some(1),
                    },
                    extent,
                );
                let view = tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D3),
                    ..Default::default()
                });

                // Dummy top-level: one sentinel entry.
                let dummy_top = BRICK_SLOT_EMPTY.to_ne_bytes();
                let top_buf = wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("SDF Top-Level Dummy Buffer"),
                        contents: &dummy_top,
                        usage: wgpu::BufferUsages::STORAGE,
                    },
                );

                // Dummy meta: has_sdf_atlas = 0.
                let meta_bytes = build_sdf_meta_bytes(
                    [0.0; 3],
                    [1.0; 3],
                    0.08,
                    8,
                    [1, 1, 1],
                    false,
                    [1, 1, 1],
                );
                let meta_buf = wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("SDF Meta Dummy Uniform"),
                        contents: &meta_bytes,
                        usage: wgpu::BufferUsages::UNIFORM,
                    },
                );

                // Dummy coarse texture: 1×1×1, value = large-positive so the
                // shader (when has_sdf_atlas=0) never reads meaningful data.
                let (coarse_tex, coarse_v) =
                    upload_coarse_texture(device, queue, [1, 1, 1], &[1.0]);

                (tex, view, top_buf, meta_buf, coarse_tex, coarse_v)
            };

        SdfResources {
            atlas_texture,
            atlas_view,
            sdf_sampler,
            top_level_buffer,
            meta_buffer,
            coarse_texture,
            coarse_view,
        }
    }

    /// Returns the wgpu bind group entries for bindings 5–9 (group 2).
    /// These borrow `self` — the caller must ensure `self` outlives the
    /// bind group.
    pub fn bind_group_entries(&self) -> Vec<wgpu::BindGroupEntry<'_>> {
        vec![
            wgpu::BindGroupEntry {
                binding: BIND_SDF_ATLAS,
                resource: wgpu::BindingResource::TextureView(&self.atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SDF_SAMPLER,
                resource: wgpu::BindingResource::Sampler(&self.sdf_sampler),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SDF_TOP_LEVEL,
                resource: self.top_level_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SDF_META,
                resource: self.meta_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SDF_COARSE,
                resource: wgpu::BindingResource::TextureView(&self.coarse_view),
            },
        ]
    }
}

/// Upload a grid-resolution coarse SDF texture (R32Float, trilinearly
/// sampled). Dimensions = `grid_dims`, data = one f32 per brick in z-y-x
/// linear order (same indexing as `SdfAtlasSection::top_level`). Handles the
/// wgpu 256-byte bytes_per_row alignment requirement by padding rows.
fn upload_coarse_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid_dims: [u32; 3],
    coarse: &[f32],
) -> (wgpu::Texture, wgpu::TextureView) {
    let [gx, gy, gz] = grid_dims;
    let extent = wgpu::Extent3d {
        width: gx.max(1),
        height: gy.max(1),
        depth_or_array_layers: gz.max(1),
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SDF Coarse Distance Texture 3D"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let bytes_per_row = (extent.width * 4).next_multiple_of(256);
    let rows_per_layer = extent.height;
    let total_bytes =
        (bytes_per_row as usize) * (rows_per_layer as usize) * (extent.depth_or_array_layers as usize);
    let mut padded = vec![0u8; total_bytes];
    let gx_usz = extent.width as usize;
    let gy_usz = extent.height as usize;
    let gz_usz = extent.depth_or_array_layers as usize;
    for z in 0..gz_usz {
        for y in 0..gy_usz {
            let row_dst = z * (bytes_per_row as usize) * (rows_per_layer as usize)
                + y * (bytes_per_row as usize);
            for x in 0..gx_usz {
                let flat = z * gy_usz * gx_usz + y * gx_usz + x;
                let f = coarse.get(flat).copied().unwrap_or(1.0);
                let dst = row_dst + x * 4;
                padded[dst..dst + 4].copy_from_slice(&f.to_ne_bytes());
            }
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(rows_per_layer),
        },
        extent,
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });
    (tex, view)
}

/// Build the 5 bind group layout entries for the SDF atlas (bindings 5–9).
pub fn sdf_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    [
        // binding 5: SDF atlas texture_3d<f32>
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SDF_ATLAS,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        },
        // binding 6: trilinear sampler
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SDF_SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // binding 7: top-level index storage buffer
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SDF_TOP_LEVEL,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // binding 8: SdfMeta uniform
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SDF_META,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // binding 9: coarse SDF texture_3d<f32> (one texel per brick)
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SDF_COARSE,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdf_meta_bytes_has_correct_size() {
        let bytes = build_sdf_meta_bytes([0.0; 3], [1.0; 3], 0.08, 8, [4, 4, 4], true, [2, 2, 2]);
        assert_eq!(bytes.len(), SDF_META_SIZE);
    }

    #[test]
    fn sdf_meta_bytes_encodes_fields() {
        let bytes = build_sdf_meta_bytes(
            [-1.0, -2.0, -3.0],
            [10.0, 20.0, 30.0],
            0.08,
            8,
            [4, 5, 6],
            true,
            [3, 7, 11],
        );
        let wx = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let wy = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let wz = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(wx, -1.0);
        assert_eq!(wy, -2.0);
        assert_eq!(wz, -3.0);

        let vs = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert!((vs - 0.08).abs() < 1e-6);

        let bsv = u32::from_ne_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(bsv, 8);

        let gx = u32::from_ne_bytes(bytes[32..36].try_into().unwrap());
        let gy = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());
        let gz = u32::from_ne_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(gx, 4);
        assert_eq!(gy, 5);
        assert_eq!(gz, 6);

        let flag = u32::from_ne_bytes(bytes[44..48].try_into().unwrap());
        assert_eq!(flag, 1);

        let ax = u32::from_ne_bytes(bytes[48..52].try_into().unwrap());
        let ay = u32::from_ne_bytes(bytes[52..56].try_into().unwrap());
        let az = u32::from_ne_bytes(bytes[56..60].try_into().unwrap());
        assert_eq!(ax, 3);
        assert_eq!(ay, 7);
        assert_eq!(az, 11);
    }

    /// Regression test for the crash at
    /// `context/plans/in-progress/lighting-foundation/crash-trace.txt`: the
    /// prior layout packed bricks linearly along Z, producing
    /// `depth = brick_size * surface_count` which exceeds wgpu's default
    /// `max_texture_dimension_3d = 2048` on any real level.
    #[test]
    fn atlas_layout_never_exceeds_max_texture_dim() {
        let brick_size = 8u32;
        let max_dim = SDF_ATLAS_MAX_TEXTURE_DIM;

        // The exact surface-brick count from the crashing
        // `assets/maps/occlusion-test.prl` bake (see crash-trace.txt:
        // "Dimension Z value 260312" → 260312 / 8 = 32539 surface bricks).
        let crashing_count = 32539u32;

        // Bonus: a comically large count that would still overwhelm a 1D
        // Z-stack but must fit within the 3D cap via cube-ish packing.
        for surface_count in [1u32, 100, 8_000, crashing_count, 1_000_000] {
            let [ax, ay, az] = atlas_brick_layout(surface_count, brick_size, max_dim);
            assert!(
                ax * brick_size <= max_dim,
                "X axis {ax}*{brick_size} exceeds {max_dim} for surface_count={surface_count}",
            );
            assert!(
                ay * brick_size <= max_dim,
                "Y axis {ay}*{brick_size} exceeds {max_dim} for surface_count={surface_count}",
            );
            assert!(
                az * brick_size <= max_dim,
                "Z axis {az}*{brick_size} exceeds {max_dim} for surface_count={surface_count}",
            );
            let capacity = (ax as u64) * (ay as u64) * (az as u64);
            assert!(
                capacity >= surface_count as u64,
                "layout [{ax},{ay},{az}] cannot hold {surface_count} bricks",
            );
        }
    }

    #[test]
    fn atlas_layout_handles_empty_and_zero_brick_size() {
        assert_eq!(atlas_brick_layout(0, 8, 2048), [1, 1, 1]);
        assert_eq!(atlas_brick_layout(100, 0, 2048), [1, 1, 1]);
    }

    #[test]
    fn dequantize_atlas_correct_scale() {
        let voxel = 0.08_f32;
        let scale = voxel / 256.0;
        // 256 units should equal exactly voxel_size_m = 0.08 m
        let atlas = vec![256i16, -256, 0, 128];
        let out = dequantize_atlas(&atlas, voxel);
        assert!((out[0] - voxel).abs() < 1e-6, "expected {voxel}, got {}", out[0]);
        assert!((out[1] + voxel).abs() < 1e-6);
        assert_eq!(out[2], 0.0);
        assert!((out[3] - 128.0 * scale).abs() < 1e-6);
    }
}
