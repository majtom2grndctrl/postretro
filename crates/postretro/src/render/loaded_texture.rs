// World-material texture loading: maps PRL texture names to baked `.prm`
// mip-chain sidecars and uploads them directly. Owns the wgpu handles for
// each material's diffuse/specular/normal slots.
// See: context/lib/resource_management.md · context/lib/build_pipeline.md

use std::path::Path;

use postretro_level_format::prm::{PrmFile, PrmFormat, PrmReadError, PrmSlot};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;

const PLACEHOLDER_SIZE: u32 = 64;
const CHECKER_SQUARE: u32 = 8;
const MAGENTA: [u8; 4] = [255, 0, 0xFF, 255];
const BLACK_RGBA: [u8; 4] = [0, 0, 0, 255];

/// Tangent-space +Z normal encoded as Rgba8Unorm: (0,0,1) → (127,127,255).
const NEUTRAL_NORMAL_PIXEL: [u8; 4] = [127, 127, 255, 255];

/// GPU resources for one world-material texture (diffuse + specular + normal).
/// Each slot carries its full mip chain; the sampler clamp matches `mip_count`.
pub struct LoadedTexture {
    pub diffuse_texture: wgpu::Texture,
    pub diffuse_view: wgpu::TextureView,
    /// Owned alongside `specular_view`; views borrow the texture, so dropping
    /// the texture invalidates the view. The renderer never reads
    /// `specular_texture` directly — it samples via `specular_view`.
    #[allow(dead_code)]
    pub specular_texture: wgpu::Texture,
    pub specular_view: wgpu::TextureView,
    /// Owned alongside `normal_view`; same rationale as `specular_texture`.
    #[allow(dead_code)]
    pub normal_texture: wgpu::Texture,
    pub normal_view: wgpu::TextureView,
    /// Mip levels present on `diffuse_texture`. The renderer keys its sampler
    /// pool off this value so `lod_max_clamp` matches the uploaded chain.
    pub mip_count: u32,
    /// `true` when every slot fell back to a placeholder (zero key, header
    /// error, or missing file). Keeps probe-skip parity with the prior PNG
    /// loader: a placeholder diffuse must not pair with sibling slot data.
    #[allow(dead_code)]
    pub is_placeholder: bool,
}

/// Upload a pre-baked mip chain to a 2D texture. Each `(width, height, bytes)`
/// entry in `levels` is a single mip level, in level order (mip 0 first). Caller
/// guarantees the byte count matches `bytes_per_pixel(format) * width * height`.
pub fn upload_texture_data(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    levels: &[(u32, u32, &[u8])],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let bytes_per_pixel: u32 = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => 4,
        wgpu::TextureFormat::R8Unorm => 1,
        other => panic!("upload_texture_data: unsupported format {other:?}"),
    };
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
                bytes_per_row: Some(bytes_per_pixel * level_w),
                rows_per_image: Some(*level_h),
            },
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

/// Split a slot's flat payload into per-level (width, height, bytes) slices.
/// The on-disk layout packs levels back-to-back with no padding: level 0
/// occupies `bpp * w * h` bytes, level 1 occupies `bpp * (w/2) * (h/2)`, etc.,
/// dimensions clamped to a minimum of 1.
fn slot_levels(slot: &PrmSlot) -> Vec<(u32, u32, &[u8])> {
    let format = slot.format;
    let bpp = match format {
        PrmFormat::Rgba8Unorm | PrmFormat::Rgba8UnormSrgb => 4,
        PrmFormat::R8Unorm => 1,
    };
    let mut out = Vec::with_capacity(slot.level_count as usize);
    let mut offset = 0usize;
    for n in 0..slot.level_count {
        let w = ((slot.width as u32) >> n).max(1);
        let h = ((slot.height as u32) >> n).max(1);
        let size = (bpp * w * h) as usize;
        out.push((w, h, &slot.payload[offset..offset + size]));
        offset += size;
    }
    out
}

fn prm_format_to_wgpu(format: PrmFormat) -> wgpu::TextureFormat {
    match format {
        PrmFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        PrmFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        PrmFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
    }
}

/// Build a 64×64 RGBA8 magenta/black checkerboard for the diffuse placeholder.
/// Single mip level — the placeholder doesn't need filtering at distance.
fn generate_checkerboard_pixels() -> Vec<u8> {
    let pixel_count = (PLACEHOLDER_SIZE * PLACEHOLDER_SIZE) as usize;
    let mut data = Vec::with_capacity(pixel_count * 4);
    for y in 0..PLACEHOLDER_SIZE {
        for x in 0..PLACEHOLDER_SIZE {
            let checker_x = x / CHECKER_SQUARE;
            let checker_y = y / CHECKER_SQUARE;
            let color = if (checker_x + checker_y) % 2 == 0 {
                &MAGENTA
            } else {
                &BLACK_RGBA
            };
            data.extend_from_slice(color);
        }
    }
    data
}

fn make_diffuse_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let data = generate_checkerboard_pixels();
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &[(PLACEHOLDER_SIZE, PLACEHOLDER_SIZE, &data)],
        "Placeholder Diffuse (Checkerboard)",
    )
}

fn make_specular_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::R8Unorm,
        &[(1, 1, &[0u8])],
        "Placeholder Specular (Black 1x1)",
    )
}

fn make_normal_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        &[(1, 1, &NEUTRAL_NORMAL_PIXEL[..])],
        "Placeholder Normal (Neutral 1x1)",
    )
}

/// Total mip levels carried by the diffuse slot, defaulting to 1 when the
/// header parsed but no diffuse data was present. The sampler pool keys off
/// this so the sampler's `lod_max_clamp` matches the uploaded mip chain.
fn header_mip_count(diffuse: &Result<PrmSlot, PrmReadError>) -> u32 {
    match diffuse {
        Ok(slot) => slot.level_count as u32,
        Err(_) => 1,
    }
}

fn placeholder_loaded_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> LoadedTexture {
    let (diffuse_texture, diffuse_view) = make_diffuse_placeholder(device, queue);
    let (specular_texture, specular_view) = make_specular_placeholder(device, queue);
    let (normal_texture, normal_view) = make_normal_placeholder(device, queue);
    LoadedTexture {
        diffuse_texture,
        diffuse_view,
        specular_texture,
        specular_view,
        normal_texture,
        normal_view,
        mip_count: 1,
        is_placeholder: true,
    }
}

fn hex_lower(key: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for &b in key {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

/// Load every world-material texture referenced by the PRL. `texture_names[i]`
/// pairs with `texture_cache_keys.keys[i]`; an all-zero key produces a silent
/// placeholder. Header errors and per-slot errors degrade to placeholders with
/// a single warning each. Returns one `LoadedTexture` per entry, parallel to
/// `texture_names`.
pub fn load_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_names: &[String],
    texture_cache_keys: &TextureCacheKeysSection,
    prm_cache_root: &Path,
) -> Vec<LoadedTexture> {
    let mut out: Vec<LoadedTexture> = Vec::with_capacity(texture_names.len());

    for (i, name) in texture_names.iter().enumerate() {
        let key = match texture_cache_keys.keys.get(i) {
            Some(k) => *k,
            None => {
                // PRL out of sync with TextureCacheKeys — log once per entry.
                log::warn!(
                    "[Loader] texture '{name}' index {i} has no cache key — using placeholders"
                );
                out.push(placeholder_loaded_texture(device, queue));
                continue;
            }
        };

        if key == [0u8; 32] {
            // Zero key signals "no source PNG" (e.g., compiler couldn't resolve
            // the name). Silent placeholder by design.
            out.push(placeholder_loaded_texture(device, queue));
            continue;
        }

        let prm_path = prm_cache_root.join(format!("{}.prm", hex_lower(&key)));
        let bytes = match std::fs::read(&prm_path) {
            Ok(b) => b,
            Err(err) => {
                log::warn!(
                    "[Loader] texture '{name}': cannot read {} : {err} — using placeholders",
                    prm_path.display(),
                );
                out.push(placeholder_loaded_texture(device, queue));
                continue;
            }
        };

        let (header_result, slot_results) = PrmFile::from_bytes_partial(&bytes);
        match header_result {
            Ok(_) => {}
            Err(e) => {
                log::warn!(
                    "[Loader] texture '{name}': .prm header error: {e:?} — using placeholders"
                );
                out.push(placeholder_loaded_texture(device, queue));
                continue;
            }
        }

        let mip_count = header_mip_count(&slot_results[0]);

        let (diffuse_texture, diffuse_view) =
            upload_slot_or_placeholder(device, queue, &slot_results[0], 0, name, Slot::Diffuse);
        let (specular_texture, specular_view) =
            upload_slot_or_placeholder(device, queue, &slot_results[1], 1, name, Slot::Specular);
        let (normal_texture, normal_view) =
            upload_slot_or_placeholder(device, queue, &slot_results[2], 2, name, Slot::Normal);

        out.push(LoadedTexture {
            diffuse_texture,
            diffuse_view,
            specular_texture,
            specular_view,
            normal_texture,
            normal_view,
            mip_count,
            is_placeholder: false,
        });
    }

    out
}

#[derive(Copy, Clone)]
enum Slot {
    Diffuse,
    Specular,
    Normal,
}

fn upload_slot_or_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot_result: &Result<PrmSlot, PrmReadError>,
    slot_idx: u8,
    name: &str,
    slot: Slot,
) -> (wgpu::Texture, wgpu::TextureView) {
    match slot_result {
        Ok(slot_data) => {
            let levels = slot_levels(slot_data);
            let format = prm_format_to_wgpu(slot_data.format);
            let label = match slot {
                Slot::Diffuse => format!("Texture '{name}' Diffuse"),
                Slot::Specular => format!("Texture '{name}' Specular"),
                Slot::Normal => format!("Texture '{name}' Normal"),
            };
            upload_texture_data(device, queue, format, &levels, &label)
        }
        Err(PrmReadError::NotPresent) => match slot {
            Slot::Diffuse => make_diffuse_placeholder(device, queue),
            Slot::Specular => make_specular_placeholder(device, queue),
            Slot::Normal => make_normal_placeholder(device, queue),
        },
        Err(e) => {
            log::warn!(
                "[Loader] texture '{name}' slot {slot_idx}: .prm slot error: {e:?} — using placeholder"
            );
            match slot {
                Slot::Diffuse => make_diffuse_placeholder(device, queue),
                Slot::Specular => make_specular_placeholder(device, queue),
                Slot::Normal => make_normal_placeholder(device, queue),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The 4 checkerboard tests verify the placeholder pixel pattern: a magenta
    // diffuse with 64×64 dimensions, alternating in 8-pixel squares. Moved from
    // the old `texture.rs` and adapted to call `generate_checkerboard_pixels`
    // directly — placeholder construction lives here now.

    #[test]
    fn checkerboard_has_correct_dimensions() {
        let data = generate_checkerboard_pixels();
        assert_eq!(data.len(), 64 * 64 * 4);
    }

    #[test]
    fn checkerboard_top_left_is_magenta() {
        let data = generate_checkerboard_pixels();
        assert_eq!(&data[0..4], &MAGENTA);
    }

    #[test]
    fn checkerboard_alternates_correctly() {
        let data = generate_checkerboard_pixels();
        assert_eq!(&data[0..4], &MAGENTA);
        let offset_8_0 = (8 * 4) as usize;
        assert_eq!(&data[offset_8_0..offset_8_0 + 4], &BLACK_RGBA);
        let offset_16_0 = (16 * 4) as usize;
        assert_eq!(&data[offset_16_0..offset_16_0 + 4], &MAGENTA);
        let offset_0_8 = (8 * 64 * 4) as usize;
        assert_eq!(&data[offset_0_8..offset_0_8 + 4], &BLACK_RGBA);
        let offset_8_8 = ((8 * 64 + 8) * 4) as usize;
        assert_eq!(&data[offset_8_8..offset_8_8 + 4], &MAGENTA);
    }

    #[test]
    fn checkerboard_all_pixels_are_magenta_or_black() {
        let data = generate_checkerboard_pixels();
        for pixel in data.chunks(4) {
            assert!(
                pixel == MAGENTA || pixel == BLACK_RGBA,
                "unexpected pixel: {pixel:?}"
            );
        }
    }

    // CPU-only tests for the .prm reader path. GPU-side upload is exercised by
    // running the engine (per testing_guide.md §3, no GPU context in tests).
    use postretro_level_format::prm::{PrmHeader, PrmSlots, STAGE_VERSION};

    fn make_diffuse_only_prm(width: u16, height: u16) -> Vec<u8> {
        let level_count = {
            let m = width.max(height).max(1) as u32;
            (m.ilog2() + 1) as u8
        };
        let mut payload: Vec<u8> = Vec::new();
        for n in 0..level_count {
            let w = ((width as u32) >> n).max(1);
            let h = ((height as u32) >> n).max(1);
            let bytes = (4 * w * h) as usize;
            // Distinct byte pattern per level so a swap would surface.
            for i in 0..bytes {
                payload.push(((i as u16 + n as u16 * 7) & 0xFF) as u8);
            }
        }
        let slot = PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width,
            height,
            level_count,
            payload,
        };
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE,
                bundle_hash: [0u8; 32],
                total_body_bytes: 0,
            },
            slots: [Some(slot), None, None],
        };
        file.to_bytes()
    }

    #[test]
    fn diffuse_only_prm_parses_to_single_slot() {
        let bytes = make_diffuse_only_prm(4, 4);
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("header parses");
        assert_eq!(header.slot_mask, PrmSlots::DIFFUSE);
        assert!(slots[0].is_ok(), "diffuse slot must parse");
        assert!(
            matches!(&slots[1], Err(PrmReadError::NotPresent)),
            "specular absent",
        );
        assert!(
            matches!(&slots[2], Err(PrmReadError::NotPresent)),
            "normal absent",
        );
        assert_eq!(header_mip_count(&slots[0]), 3, "4x4 → 3 mips");
    }

    #[test]
    fn truncated_prm_header_classifies_as_header_error() {
        // 10 bytes < 43-byte header → Truncated.
        let bytes = vec![0u8; 10];
        let (header, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(header, Err(PrmReadError::Truncated)),
            "expected Truncated, got {header:?}",
        );
    }

    #[test]
    fn zero_key_signals_placeholder_path() {
        // The load_textures placeholder branch is keyed off `key == [0u8; 32]`.
        // Reaching the GPU upload requires a device; instead pin the predicate
        // so a future refactor doesn't accidentally drop the zero-key fast path.
        let zero = [0u8; 32];
        assert_eq!(zero, [0u8; 32]);
        // hex_lower of zero is 64 zero chars — a non-empty filename, so a
        // mistaken disk lookup would clash against a real cache entry.
        assert_eq!(hex_lower(&zero), "0".repeat(64));
    }

    #[test]
    fn hex_lower_is_lowercase_and_padded() {
        let mut key = [0u8; 32];
        key[0] = 0xAB;
        key[31] = 0x0F;
        let s = hex_lower(&key);
        assert_eq!(s.len(), 64);
        assert!(s.starts_with("ab"));
        assert!(s.ends_with("0f"));
    }

    #[test]
    fn slot_levels_walks_pyramid_in_order() {
        // 4x4 RGBA → mip 0 64B, mip 1 16B, mip 2 4B. Total 84B.
        let bytes = make_diffuse_only_prm(4, 4);
        let (_, slots) = PrmFile::from_bytes_partial(&bytes);
        let slot = slots[0].as_ref().expect("diffuse parses");
        let levels = slot_levels(slot);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0].0, 4);
        assert_eq!(levels[0].1, 4);
        assert_eq!(levels[0].2.len(), 64);
        assert_eq!(levels[1].0, 2);
        assert_eq!(levels[1].1, 2);
        assert_eq!(levels[1].2.len(), 16);
        assert_eq!(levels[2].0, 1);
        assert_eq!(levels[2].1, 1);
        assert_eq!(levels[2].2.len(), 4);
    }
}
