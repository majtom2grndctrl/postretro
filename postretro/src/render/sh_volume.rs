// SH irradiance volume GPU resources: 3D texture upload, sampler, grid-info
// uniform, and bind group (group 3).
//
// See: context/plans/in-progress/lighting-foundation/6-sh-volume.md
//      context/lib/rendering_pipeline.md §4

use postretro_level_format::sh_volume::{ShProbe, ShVolumeSection};

/// Number of SH L2 bands (= number of 3D textures we bind). Each band stores
/// its RGB coefficients in the `.rgb` channels of an `Rgba16Float` 3D texture
/// sized to the probe grid. `.a` is unused padding.
///
/// 9 textures + 1 sampler + 1 uniform = 11 bindings in group 3, well under
/// wgpu's default `max_sampled_textures_per_shader_stage` limit.
pub const SH_BAND_COUNT: usize = 9;

/// Byte size of `ShGridInfo` — four `vec4` slots to satisfy std140 alignment
/// rules (vec3 fields align to 16, followed by a same-slot scalar).
///
/// Layout (must match the WGSL `ShGridInfo` struct in `forward.wgsl`):
///   0..12   grid_origin       (vec3<f32>)
///   12..16  has_sh_volume     (u32, 0 or 1)
///   16..28  cell_size         (vec3<f32>)
///   28..32  _pad0             (u32)
///   32..44  grid_dimensions   (vec3<u32>)
///   44..48  _pad1             (u32)
pub const SH_GRID_INFO_SIZE: usize = 48;

/// Uploaded SH volume handles + bind group. Always populated — when the level
/// has no SH section, the bind group binds dummy 1×1×1 textures and the
/// `has_sh_volume` flag is zero so the fragment shader skips SH sampling.
pub struct ShVolumeResources {
    pub bind_group: wgpu::BindGroup,
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Whether a real SH volume was uploaded (false => dummy / ambient fallback).
    /// Read by the diagnostic logging path; kept public for future debug UI.
    #[allow(dead_code)]
    pub present: bool,
}

impl ShVolumeResources {
    /// Build group 3 (SH volume) resources. `section` is `None` when the PRL
    /// file had no `ShVolume` section — in that case dummy 1×1×1 textures are
    /// uploaded and the `has_sh_volume` flag is zero so the shader skips SH
    /// sampling and falls back to `ambient_floor + direct_sum`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&ShVolumeSection>,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH Volume Bind Group Layout"),
            entries: &sh_bind_group_layout_entries(),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SH Volume Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Decide whether we have a usable SH volume. A zero-dimension grid is
        // treated the same as a missing section — nothing to sample.
        let usable = section.filter(|s| {
            s.grid_dimensions[0] > 0 && s.grid_dimensions[1] > 0 && s.grid_dimensions[2] > 0
        });

        let grid_origin: [f32; 3];
        let cell_size: [f32; 3];
        let grid_dimensions: [u32; 3];
        let present: bool;
        let textures: Vec<wgpu::Texture>;

        if let Some(sec) = usable {
            let packed = pack_probes_to_band_slices(&sec.probes, sec.grid_dimensions);
            textures = (0..SH_BAND_COUNT)
                .map(|band| {
                    upload_band_texture(device, queue, sec.grid_dimensions, &packed[band], band)
                })
                .collect();
            grid_origin = sec.grid_origin;
            cell_size = sec.cell_size;
            grid_dimensions = sec.grid_dimensions;
            present = true;
        } else {
            let dummy = [0u16; 4]; // one rgba16float texel, all zeros.
            textures = (0..SH_BAND_COUNT)
                .map(|band| upload_band_texture(device, queue, [1, 1, 1], &dummy, band))
                .collect();
            grid_origin = [0.0; 3];
            cell_size = [1.0; 3];
            grid_dimensions = [1, 1, 1];
            present = false;
        }

        // Upload grid-info uniform.
        let grid_info_bytes = build_grid_info_bytes(grid_origin, cell_size, grid_dimensions, present);
        let grid_info_buffer = device.create_buffer_init_helper(
            "SH Grid Info Uniform",
            &grid_info_bytes,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();

        let mut entries: Vec<wgpu::BindGroupEntry> = Vec::with_capacity(SH_BAND_COUNT + 2);
        entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
        for (i, view) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: (1 + SH_BAND_COUNT) as u32,
            resource: grid_info_buffer.as_entire_binding(),
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Volume Bind Group"),
            layout: &bind_group_layout,
            entries: &entries,
        });

        // The textures/sampler/buffer are held alive via the bind group's
        // internal Arc references (wgpu caches descriptor resources).
        Self {
            bind_group,
            bind_group_layout,
            present,
        }
    }
}

// --- Helpers ---

fn sh_bind_group_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::with_capacity(SH_BAND_COUNT + 2);
    // binding 0: sampler
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    });
    // bindings 1..=SH_BAND_COUNT: 3D textures
    for i in 0..SH_BAND_COUNT {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 1 + i as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        });
    }
    // binding 1 + SH_BAND_COUNT: ShGridInfo uniform
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: (1 + SH_BAND_COUNT) as u32,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries
}

/// Repack `ShProbe.sh_coefficients` from per-probe, band-interleaved RGB
/// (`[b0_r, b0_g, b0_b, b1_r, ...]`) into 9 per-band byte buffers, each sized
/// `grid_x × grid_y × grid_z × 8 bytes` (one `Rgba16Float` texel per probe).
///
/// Invalid probes (`validity == 0`) upload as all-zero coefficients so the
/// hardware trilinear filter blends them towards darkness near walls — this
/// matches the baker's contract and removes the need for a shader-side
/// validity branch.
fn pack_probes_to_band_slices(
    probes: &[ShProbe],
    grid: [u32; 3],
) -> Vec<Vec<u16>> {
    let total = (grid[0] as usize) * (grid[1] as usize) * (grid[2] as usize);
    debug_assert_eq!(probes.len(), total);

    // Each band's buffer holds 4 u16 halves per probe (R, G, B, pad=0).
    let mut bands: Vec<Vec<u16>> = (0..SH_BAND_COUNT)
        .map(|_| vec![0u16; total * 4])
        .collect();

    for (probe_idx, probe) in probes.iter().enumerate() {
        let off = probe_idx * 4;
        if probe.validity == 0 {
            // Already zero-initialized; leave invalid probes dark.
            continue;
        }
        for (band, band_buf) in bands.iter_mut().enumerate() {
            let r = probe.sh_coefficients[band * 3];
            let g = probe.sh_coefficients[band * 3 + 1];
            let b = probe.sh_coefficients[band * 3 + 2];
            band_buf[off] = f32_to_f16_bits(r);
            band_buf[off + 1] = f32_to_f16_bits(g);
            band_buf[off + 2] = f32_to_f16_bits(b);
            band_buf[off + 3] = 0;
        }
    }

    bands
}

/// Create a `Rgba16Float` 3D texture sized to `grid` and upload `data`
/// (row-major by x, then y, then z — matching the baker's z-major iteration
/// order after reshaping).
fn upload_band_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid: [u32; 3],
    data_u16: &[u16],
    band: usize,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: grid[0].max(1),
        height: grid[1].max(1),
        depth_or_array_layers: grid[2].max(1),
    };

    let label = format!("SH Volume Band {band}");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    // Safe reinterpretation: Rgba16Float wants 8 bytes per texel (4 halves).
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
    present: bool,
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
    bytes
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
        let bytes = build_grid_info_bytes([1.5, 2.5, 3.5], [0.25, 0.5, 1.0], [4, 5, 6], true);
        assert_eq!(bytes.len(), SH_GRID_INFO_SIZE);

        let ox = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let oy = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let oz = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let flag = u32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let cx = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        let gy = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());

        assert_eq!([ox, oy, oz], [1.5, 2.5, 3.5]);
        assert_eq!(flag, 1);
        assert_eq!(cx, 0.25);
        assert_eq!(gy, 5);
    }

    #[test]
    fn grid_info_flag_zero_when_absent() {
        let bytes = build_grid_info_bytes([0.0; 3], [1.0; 3], [1, 1, 1], false);
        let flag = u32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(flag, 0);
    }

    #[test]
    fn pack_probes_zeroes_invalid() {
        let probe_valid = ShProbe {
            sh_coefficients: core::array::from_fn(|i| (i + 1) as f32),
            validity: 1,
        };
        let probe_invalid = ShProbe {
            sh_coefficients: [999.0; 27],
            validity: 0,
        };
        let bands = pack_probes_to_band_slices(&[probe_valid, probe_invalid], [2, 1, 1]);
        assert_eq!(bands.len(), SH_BAND_COUNT);

        // Valid probe: band 0, probe 0 should encode coefficients [1, 2, 3].
        let b0 = &bands[0];
        // First texel: rgba at offsets 0..4.
        let r = b0[0];
        let g = b0[1];
        let b = b0[2];
        let a = b0[3];
        assert_eq!(r, f32_to_f16_bits(1.0));
        assert_eq!(g, f32_to_f16_bits(2.0));
        assert_eq!(b, f32_to_f16_bits(3.0));
        assert_eq!(a, 0);

        // Invalid probe: everything must be zero.
        assert_eq!(b0[4], 0);
        assert_eq!(b0[5], 0);
        assert_eq!(b0[6], 0);
        assert_eq!(b0[7], 0);

        // Higher band on valid probe encodes next RGB triplet.
        let b1 = &bands[1];
        assert_eq!(b1[0], f32_to_f16_bits(4.0));
        assert_eq!(b1[1], f32_to_f16_bits(5.0));
        assert_eq!(b1[2], f32_to_f16_bits(6.0));
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
        // band 0, all three channels
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
        assert!((up - down).abs() > 0.1, "directional contrast too weak: up={up}, down={down}");
    }
}
