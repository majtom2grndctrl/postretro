// Texture upload primitives: `.prm` row layouts, 2D and array mip-chain
// uploads, and the `.prm`-to-wgpu format map.
// See: context/lib/resource_management.md

use postretro_level_format::prm::PrmFormat;

/// Row-layout classification for a `.prm`-backed upload format.
///
/// `Some(bpp)` is an uncompressed format with that bytes-per-pixel;
/// `None` is block-compressed (BC5), sized block-by-block instead.
/// `upload_texture_data` and `upload_texture_array_data` share this one table,
/// so a format legal in a `.prm` cannot be uploadable through one and a panic
/// through the other.
///
/// Returns `None` for a format outside the `.prm` contract, which is a caller
/// bug — both uploaders turn that into a panic naming the format. Split out
/// from the uploaders so the mapping is checkable without a GPU.
pub(super) fn upload_bytes_per_pixel(format: wgpu::TextureFormat) -> Option<Option<u32>> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(Some(4)),
        // The two-channel surface map: R specular, G inverted height. A material with an
        // `_h.png` height sibling bakes its specular slot to this instead of
        // `R8Unorm`, so every world-material upload path must size it.
        wgpu::TextureFormat::Rg8Unorm => Some(Some(2)),
        wgpu::TextureFormat::R8Unorm => Some(Some(1)),
        wgpu::TextureFormat::Bc5RgUnorm => Some(None),
        _ => None,
    }
}

/// Upload a pre-baked mip chain to a 2D texture. Each `(width, height, bytes)`
/// entry in `levels` is a single mip level, in level order (mip 0 first), with
/// `width`/`height` the LOGICAL mip dimensions.
///
/// For uncompressed formats (Rgba8*, Rg8, R8) the byte count must equal
/// `bytes_per_pixel(format) * width * height`, uploaded with
/// `bytes_per_row = bytes_per_pixel * width` and `rows_per_image = height`.
///
/// For BC5 (block-compressed, 16 bytes per 4×4 texel block) the byte count is
/// the block-aligned `ceil(width/4) * ceil(height/4) * 16`, uploaded with
/// `bytes_per_row = ceil(width/4) * 16` (one block row) and
/// `rows_per_image = ceil(height/4)` (block rows). The copy extent stays the
/// logical `width × height`; wgpu permits a block-compressed copy whose extent
/// equals the mip level size even when not a multiple of the 4×4 block.
pub fn upload_texture_data(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    levels: &[(u32, u32, &[u8])],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    // `None` = block-compressed (BC5); `Some(bpp)` = uncompressed with that
    // bytes-per-pixel. Drives the per-level `bytes_per_row`/`rows_per_image`.
    let bytes_per_pixel: Option<u32> = upload_bytes_per_pixel(format)
        .unwrap_or_else(|| panic!("upload_texture_data: unsupported format {format:?}"));
    let (mip0_w, mip0_h, _) = levels
        .first()
        .copied()
        .expect("upload_texture_data: levels must contain at least mip 0");
    let mip_level_count = levels.len() as u32;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: mip0_w,
            height: mip0_h,
            depth_or_array_layers: 1,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    for (level, (level_w, level_h, bytes)) in levels.iter().enumerate() {
        // Uncompressed: one row = bpp*w bytes, height rows.
        // BC5: one block row = ceil(w/4) blocks × 16 bytes; ceil(h/4) block rows.
        let (bytes_per_row, rows_per_image) = match bytes_per_pixel {
            Some(bpp) => (bpp * level_w, *level_h),
            None => (level_w.div_ceil(4) * 16, level_h.div_ceil(4)),
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(rows_per_image),
            },
            // Copy extent stays the LOGICAL mip size; wgpu allows a block-
            // compressed copy whose extent equals the logical mip dims even when
            // not a multiple of 4×4 (WebGPU physical-vs-logical size rule, GPUImageCopyTexture).
            wgpu::Extent3d {
                width: *level_w,
                height: *level_h,
                depth_or_array_layers: 1,
            },
        );
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Upload layer-major mip chains to a `texture_2d_array`.
///
/// `layers` is ordered by array layer; each inner vector is ordered by mip
/// level. The explicit counts come from the CPU-side upload plan and are
/// asserted against the payload shape before the GPU descriptor is built.
/// Logical dimensions and BC5 copy layout follow `upload_texture_data`.
pub fn upload_texture_array_data(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    layers: &[Vec<(u32, u32, &[u8])>],
    array_layer_count: u32,
    mip_level_count: u32,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let bytes_per_pixel: Option<u32> = upload_bytes_per_pixel(format)
        .unwrap_or_else(|| panic!("upload_texture_array_data: unsupported format {format:?}"));
    assert_eq!(
        layers.len(),
        array_layer_count as usize,
        "upload_texture_array_data: layer payload count must match array_layer_count"
    );
    let first_layer = layers
        .first()
        .expect("upload_texture_array_data: layers must contain at least layer 0");
    assert_eq!(
        first_layer.len(),
        mip_level_count as usize,
        "upload_texture_array_data: layer 0 mip count must match mip_level_count"
    );
    let (mip0_w, mip0_h, _) = first_layer
        .first()
        .copied()
        .expect("upload_texture_array_data: each layer must contain mip 0");

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: mip0_w,
            height: mip0_h,
            depth_or_array_layers: array_layer_count,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    for (layer, levels) in layers.iter().enumerate() {
        assert_eq!(
            levels.len(),
            mip_level_count as usize,
            "upload_texture_array_data: every layer must have the planned mip count"
        );
        for (level, (level_w, level_h, bytes)) in levels.iter().enumerate() {
            let (bytes_per_row, rows_per_image) = match bytes_per_pixel {
                Some(bpp) => (bpp * level_w, *level_h),
                None => (level_w.div_ceil(4) * 16, level_h.div_ceil(4)),
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(rows_per_image),
                },
                wgpu::Extent3d {
                    width: *level_w,
                    height: *level_h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(&format!("{label} View")),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        mip_level_count: Some(mip_level_count),
        array_layer_count: Some(array_layer_count),
        ..Default::default()
    });
    (texture, view)
}

pub(in crate::render) fn prm_format_to_wgpu(format: PrmFormat) -> wgpu::TextureFormat {
    match format {
        PrmFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        PrmFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        PrmFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
        // Two-channel surface map: R specular, G inverted height (see `PrmFormat`).
        PrmFormat::Rg8Unorm => wgpu::TextureFormat::Rg8Unorm,
        // BC5 two-channel (R,G) block-compressed normal map. Requires the
        // adapter's TEXTURE_COMPRESSION_BC feature (checked at device creation
        // in render/mod.rs).
        PrmFormat::Bc5RgUnorm => wgpu::TextureFormat::Bc5RgUnorm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::prm::PrmSlot;
    use postretro_render_cpu::loaded_texture::slot_levels;

    /// The renderer must be able to upload every format a `.prm` may legally
    /// carry. `Rg8Unorm` is legal on the specular slot, but for a while the
    /// uploaders' row-layout table did not know it — a world material with an
    /// `_h.png` sibling panicked at level load instead of uploading.
    #[test]
    fn every_prm_format_has_an_upload_row_layout() {
        for format in [
            PrmFormat::Rgba8UnormSrgb,
            PrmFormat::Rgba8Unorm,
            PrmFormat::R8Unorm,
            PrmFormat::Rg8Unorm,
            PrmFormat::Bc5RgUnorm,
        ] {
            assert!(
                upload_bytes_per_pixel(prm_format_to_wgpu(format)).is_some(),
                "{format:?} is representable in a .prm but has no upload row layout",
            );
        }
        // The surface map is two bytes per texel — twice the single-channel
        // specular slot it replaces, and the byte count the upload must write.
        assert_eq!(
            prm_format_to_wgpu(PrmFormat::Rg8Unorm),
            wgpu::TextureFormat::Rg8Unorm,
        );
        assert_eq!(
            upload_bytes_per_pixel(prm_format_to_wgpu(PrmFormat::Rg8Unorm)),
            Some(Some(2)),
        );
        // BC5 stays block-sized, not per-pixel.
        assert_eq!(
            upload_bytes_per_pixel(prm_format_to_wgpu(PrmFormat::Bc5RgUnorm)),
            Some(None),
        );
        // A format outside the .prm contract is still reported as unsupported,
        // so the uploaders keep panicking on a caller bug rather than
        // guessing a row stride.
        assert_eq!(
            upload_bytes_per_pixel(wgpu::TextureFormat::Rgba16Float),
            None,
        );
    }

    /// A surface-map specular slot must slice into upload ranges that exactly
    /// consume its payload — the CPU-side half of "would upload the right byte
    /// count", provable without a device.
    #[test]
    fn surface_map_specular_slot_slices_to_its_whole_payload() {
        let (width, height) = (64u16, 32u16);
        let level_count = postretro_level_format::prm::expected_level_count(width, height);
        let payload_len: usize = (0..level_count)
            .map(|level| {
                let w = (u32::from(width) >> level).max(1);
                let h = (u32::from(height) >> level).max(1);
                2 * (w * h) as usize
            })
            .sum();
        let slot = PrmSlot {
            format: PrmFormat::Rg8Unorm,
            width,
            height,
            level_count,
            payload: vec![0u8; payload_len],
        };

        let levels = slot_levels(&slot);
        assert_eq!(levels.len(), usize::from(level_count));
        assert_eq!(
            (levels[0].0, levels[0].1, levels[0].2.len()),
            (64, 32, 4096)
        );

        let bpp = upload_bytes_per_pixel(prm_format_to_wgpu(slot.format))
            .expect("surface map is uploadable")
            .expect("surface map is uncompressed");
        let uploaded: usize = levels
            .iter()
            .map(|(w, h, bytes)| {
                // What `upload_texture_data` writes per level: bytes_per_row *
                // rows_per_image, which must be exactly the slice it was given.
                assert_eq!(bytes.len(), (bpp * w * h) as usize);
                bytes.len()
            })
            .sum();
        assert_eq!(
            uploaded, payload_len,
            "the per-level uploads must consume the slot payload exactly",
        );
    }
}
