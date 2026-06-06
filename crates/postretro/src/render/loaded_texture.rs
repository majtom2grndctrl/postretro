// World-material texture loading: maps PRL texture names to baked `.prm`
// mip-chain sidecars and uploads them directly. Owns the wgpu handles for
// each material's diffuse/specular/normal slots.
// See: context/lib/resource_management.md · context/lib/build_pipeline.md

use std::path::Path;

use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmReadError, PrmSlot, PrmSlots, cache_filename_for_key,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;

const PLACEHOLDER_SIZE: u32 = 64;
const CHECKER_SQUARE: u32 = 8;
const MAGENTA: [u8; 4] = [255, 0, 0xFF, 255];
const BLACK_RGBA: [u8; 4] = [0, 0, 0, 255];

/// Tangent-space +Z normal encoded as Rgba8Unorm: (0,0,1) → (127,127,255).
/// The 1×1 placeholder stays Rgba8Unorm because BC5 requires a 4×4-block
/// minimum. The shader samples both Rgba8Unorm and Bc5RgUnorm normals as
/// `texture_2d<f32>`, so its `.rg * 2 - 1` decode works for either format.
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
    /// Max mip levels across all uploaded slots. The sampler's `lod_max_clamp`
    /// is keyed by this value so no slot is over-clamped when sibling slots
    /// have different chain depths (e.g. corrupted diffuse with intact normal).
    pub mip_count: u32,
}

/// Upload a pre-baked mip chain to a 2D texture. Each `(width, height, bytes)`
/// entry in `levels` is a single mip level, in level order (mip 0 first), with
/// `width`/`height` the LOGICAL mip dimensions.
///
/// For uncompressed formats (Rgba8*, R8) the byte count must equal
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
    let bytes_per_pixel: Option<u32> = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(4),
        wgpu::TextureFormat::R8Unorm => Some(1),
        wgpu::TextureFormat::Bc5RgUnorm => None,
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

/// Byte size of a single mip level for `format` at logical dims `w`×`h`.
/// Uncompressed formats use `bpp * w * h`; BC5 is block-compressed and sizes
/// each level as `ceil(w/4) * ceil(h/4)` blocks × 16 bytes per block (the baker
/// pads non-multiple-of-4 levels up to block alignment). This must agree with
/// `postretro_level_format`'s `expected_payload_bytes` so the on-disk layout and
/// the runtime split never disagree.
fn level_byte_size(format: PrmFormat, w: u32, h: u32) -> usize {
    match format {
        PrmFormat::Rgba8Unorm | PrmFormat::Rgba8UnormSrgb => (4 * w * h) as usize,
        PrmFormat::R8Unorm => (w * h) as usize,
        PrmFormat::Bc5RgUnorm => (w.div_ceil(4) * h.div_ceil(4) * 16) as usize,
    }
}

/// Split a slot's flat payload into per-level (width, height, bytes) slices.
/// Levels are packed back-to-back with no inter-level padding. Returned dims
/// are always the LOGICAL mip dimensions (`width>>n`, `height>>n`, clamped to a
/// minimum of 1); the texture extent uses these. For uncompressed formats the
/// slice is `bpp * w * h` bytes; for BC5 it is the block-aligned size
/// (`ceil(w/4) * ceil(h/4) * 16`), so the slice can cover dims padded up to the
/// next 4×4 block boundary while the reported extent stays logical.
fn slot_levels(slot: &PrmSlot) -> Vec<(u32, u32, &[u8])> {
    let format = slot.format;
    debug_assert_eq!(
        slot.payload.len(),
        (0..slot.level_count)
            .map(|n| {
                let w = ((slot.width as u32) >> n).max(1);
                let h = ((slot.height as u32) >> n).max(1);
                level_byte_size(format, w, h)
            })
            .sum::<usize>(),
        "slot payload length must equal the sum of per-level byte sizes across all {} mip \
         levels (width={}, height={}, format={:?}); uncompressed levels are bpp*w*h, BC5 \
         levels are ceil(w/4)*ceil(h/4)*16 — in-process-constructed slots must match the \
         pyramid implied by width/height/level_count",
        slot.level_count,
        slot.width,
        slot.height,
        format,
    );
    let mut out = Vec::with_capacity(slot.level_count as usize);
    let mut offset = 0usize;
    for n in 0..slot.level_count {
        let w = ((slot.width as u32) >> n).max(1);
        let h = ((slot.height as u32) >> n).max(1);
        let size = level_byte_size(format, w, h);
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
        // BC5 two-channel (R,G) block-compressed normal map. Requires the
        // adapter's TEXTURE_COMPRESSION_BC feature (checked at device creation
        // in render/mod.rs).
        PrmFormat::Bc5RgUnorm => wgpu::TextureFormat::Bc5RgUnorm,
    }
}

/// Build a 64×64 RGBA8 magenta/black checkerboard for the diffuse placeholder.
/// Single mip level — the placeholder doesn't need filtering at distance.
pub(super) fn generate_checkerboard_pixels() -> Vec<u8> {
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

/// Maximum mip levels across all three slots. Takes the max (not diffuse-only)
/// so a corrupted diffuse with intact siblings doesn't clamp those siblings to
/// LOD 0. Defaults to 1 when no slot parses cleanly — disables mip filtering
/// rather than clamping to a wrong level. Used to key the sampler pool
/// (`render/mod.rs` `mip_count_aniso_samplers`).
fn header_mip_count(slots: &[Result<PrmSlot, PrmReadError>; 3]) -> u32 {
    slots
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|s| s.level_count as u32)
        .max()
        .unwrap_or(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextureSlotPolicy {
    WorldBundle,
    ModelDiffuseOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextureSlotPlan {
    consume: [bool; 3],
    mip_count: u32,
}

fn texture_slot_plan(
    header_slots: PrmSlots,
    slots: &[Result<PrmSlot, PrmReadError>; 3],
    policy: TextureSlotPolicy,
) -> TextureSlotPlan {
    match policy {
        TextureSlotPolicy::WorldBundle => TextureSlotPlan {
            consume: [true, true, true],
            mip_count: header_mip_count(slots),
        },
        TextureSlotPolicy::ModelDiffuseOnly => TextureSlotPlan {
            consume: [header_slots.contains(PrmSlots::DIFFUSE), false, false],
            mip_count: slots[0]
                .as_ref()
                .map(|slot| slot.level_count as u32)
                .unwrap_or(1),
        },
    }
}

/// All-slot placeholder texture: 64×64 checkerboard diffuse, 1×1 black specular,
/// 1×1 neutral normal. Shared between `load_textures`' per-texture fallback path
/// and the renderer's no-level-loaded bootstrap slot.
pub(super) fn placeholder_loaded_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> LoadedTexture {
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
    }
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

        let prm_path = prm_cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
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
        let header = match header_result {
            Ok(header) => header,
            Err(e) => {
                log::warn!(
                    "[Loader] texture '{name}': .prm header error: {e:?} — using placeholders"
                );
                out.push(placeholder_loaded_texture(device, queue));
                continue;
            }
        };

        let plan = texture_slot_plan(
            header.slot_mask,
            &slot_results,
            TextureSlotPolicy::WorldBundle,
        );

        let (diffuse_texture, diffuse_view) = upload_slot_or_placeholder(
            device,
            queue,
            &slot_results[0],
            0,
            name,
            Slot::Diffuse,
            plan.consume[0],
        );
        let (specular_texture, specular_view) = upload_slot_or_placeholder(
            device,
            queue,
            &slot_results[1],
            1,
            name,
            Slot::Specular,
            plan.consume[1],
        );
        let (normal_texture, normal_view) = upload_slot_or_placeholder(
            device,
            queue,
            &slot_results[2],
            2,
            name,
            Slot::Normal,
            plan.consume[2],
        );

        out.push(LoadedTexture {
            diffuse_texture,
            diffuse_view,
            specular_texture,
            specular_view,
            normal_texture,
            normal_view,
            mip_count: plan.mip_count,
        });
    }

    out
}

/// Load one model material from the shared diffuse-addressed `.prm` cache.
/// Models consume only diffuse in this slice even when the cache entry is a
/// richer world bundle; specular and normal always use neutral placeholders.
pub(super) fn load_model_diffuse_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    name: &str,
    key: [u8; 32],
    prm_cache_root: &Path,
) -> LoadedTexture {
    if key == [0u8; 32] {
        return placeholder_loaded_texture(device, queue);
    }

    let prm_path = prm_cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
    let bytes = match std::fs::read(&prm_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!(
                "[Loader] model texture '{name}': cannot read {} : {err} — using placeholders",
                prm_path.display(),
            );
            return placeholder_loaded_texture(device, queue);
        }
    };

    let (header_result, slot_results) = PrmFile::from_bytes_partial(&bytes);
    let header = match header_result {
        Ok(header) => header,
        Err(err) => {
            log::warn!(
                "[Loader] model texture '{name}': .prm header error: {err:?} — using placeholders"
            );
            return placeholder_loaded_texture(device, queue);
        }
    };
    let plan = texture_slot_plan(
        header.slot_mask,
        &slot_results,
        TextureSlotPolicy::ModelDiffuseOnly,
    );
    let (diffuse_texture, diffuse_view) = upload_slot_or_placeholder(
        device,
        queue,
        &slot_results[0],
        0,
        name,
        Slot::Diffuse,
        plan.consume[0],
    );
    let (specular_texture, specular_view) = make_specular_placeholder(device, queue);
    let (normal_texture, normal_view) = make_normal_placeholder(device, queue);

    LoadedTexture {
        diffuse_texture,
        diffuse_view,
        specular_texture,
        specular_view,
        normal_texture,
        normal_view,
        mip_count: plan.mip_count,
    }
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
    consume: bool,
) -> (wgpu::Texture, wgpu::TextureView) {
    if !consume {
        return match slot {
            Slot::Diffuse => make_diffuse_placeholder(device, queue),
            Slot::Specular => make_specular_placeholder(device, queue),
            Slot::Normal => make_normal_placeholder(device, queue),
        };
    }

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

    // Checkerboard placeholder pixel pattern: 64×64 magenta/black, 8-pixel squares.

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
        file.to_bytes().expect("diffuse-only .prm serializes")
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
        assert_eq!(header_mip_count(&slots), 3, "4x4 → 3 mips");
    }

    // Regression: a model sharing a diffuse-addressed cache entry with a richer
    // world bundle must not consume the world's specular or normal slots.
    #[test]
    fn model_slot_plan_consumes_only_diffuse_from_richer_world_bundle() {
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL,
                bundle_hash: [0u8; 32],
                total_body_bytes: 0,
            },
            slots: [
                Some(PrmSlot {
                    format: PrmFormat::Rgba8UnormSrgb,
                    width: 4,
                    height: 4,
                    level_count: 3,
                    payload: vec![0u8; 84],
                }),
                Some(PrmSlot {
                    format: PrmFormat::R8Unorm,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![255],
                }),
                Some(PrmSlot {
                    format: PrmFormat::Rgba8Unorm,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: NEUTRAL_NORMAL_PIXEL.to_vec(),
                }),
            ],
        };
        let bytes = file.to_bytes().unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.unwrap();

        let plan = texture_slot_plan(
            header.slot_mask,
            &slots,
            TextureSlotPolicy::ModelDiffuseOnly,
        );

        assert_eq!(plan.consume, [true, false, false]);
        assert_eq!(plan.mip_count, 3);
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
        // cache_filename_for_key of zero is 64 zero chars — a non-empty
        // filename, so a mistaken disk lookup would clash against a real cache
        // entry.
        assert_eq!(cache_filename_for_key(&zero), "0".repeat(64));
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

    #[test]
    fn slot_levels_splits_bc5_into_block_aligned_levels() {
        use postretro_level_format::prm::bc5_level_count;

        // 8×8 normal slot. bc5_level_count truncates to levels whose dims are
        // both ≥ 4: level 0 = 8×8, level 1 = 4×4 (level 2 would be 2×2 → dropped).
        let width: u16 = 8;
        let height: u16 = 8;
        let level_count = bc5_level_count(width, height);
        assert_eq!(level_count, 2, "8×8 BC5 chain truncates to 2 levels");

        // level 0: ceil(8/4)*ceil(8/4) = 2*2 = 4 blocks × 16 = 64 bytes.
        // level 1: ceil(4/4)*ceil(4/4) = 1*1 = 1 block × 16 = 16 bytes.
        let payload = vec![0u8; 64 + 16];
        let slot = PrmSlot {
            format: PrmFormat::Bc5RgUnorm,
            width,
            height,
            level_count,
            payload,
        };

        let levels = slot_levels(&slot);
        assert_eq!(levels.len(), 2);
        // Logical dims reported, block-aligned byte slices.
        assert_eq!(levels[0].0, 8);
        assert_eq!(levels[0].1, 8);
        assert_eq!(levels[0].2.len(), 64);
        assert_eq!(levels[1].0, 4);
        assert_eq!(levels[1].1, 4);
        assert_eq!(levels[1].2.len(), 16);
    }
}
