// GPU texture resources and baked `.prm` upload for world and model materials.
// See: context/lib/resource_management.md · context/lib/rendering_pipeline.md

use std::path::Path;

use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmReadError, PrmSlot, cache_filename_for_key,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_render_cpu::loaded_texture::{
    TextureSlotPlan, TextureSlotPolicy, slot_levels, texture_slot_plan,
};

const PLACEHOLDER_SIZE: u32 = 64;
const CHECKER_SQUARE: u32 = 8;
const MAGENTA: [u8; 4] = [255, 0, 0xFF, 255];
const BLACK_RGBA: [u8; 4] = [0, 0, 0, 255];

/// Tangent-space +Z normal encoded as Rgba8Unorm: (0,0,1) → (127,127,255).
/// The 1×1 placeholder stays Rgba8Unorm because BC5 requires a 4×4-block
/// minimum. The shader samples both Rgba8Unorm and Bc5RgUnorm normals as
/// `texture_2d<f32>`, so its `.rg * 2 - 1` decode works for either format.
const NEUTRAL_NORMAL_PIXEL: [u8; 4] = [127, 127, 255, 255];

/// GPU resources for one world or model material.
/// World loading consumes all available slots; model loading consumes diffuse
/// only. Each uploaded slot carries its full mip chain.
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
    /// Owned alongside `emissive_view`; the black sRGB placeholder preserves the
    /// additive-of-zero contract when an `_e.png` sibling is absent.
    #[allow(dead_code)]
    pub emissive_texture: wgpu::Texture,
    pub emissive_view: wgpu::TextureView,
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
    let bytes_per_pixel: Option<u32> = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(4),
        wgpu::TextureFormat::R8Unorm => Some(1),
        wgpu::TextureFormat::Bc5RgUnorm => None,
        other => panic!("upload_texture_array_data: unsupported format {other:?}"),
    };
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

pub(super) fn prm_format_to_wgpu(format: PrmFormat) -> wgpu::TextureFormat {
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

fn make_emissive_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &[(1, 1, &BLACK_RGBA)],
        "Placeholder Emissive (Black sRGB 1x1)",
    )
}

/// All-slot placeholder texture: 64×64 checkerboard diffuse, 1×1 black specular,
/// 1×1 neutral normal, and 1×1 black sRGB emissive. Shared between `load_textures`' per-texture fallback path
/// and the renderer's no-level-loaded bootstrap slot.
pub(super) fn placeholder_loaded_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> LoadedTexture {
    let (diffuse_texture, diffuse_view) = make_diffuse_placeholder(device, queue);
    let (specular_texture, specular_view) = make_specular_placeholder(device, queue);
    let (normal_texture, normal_view) = make_normal_placeholder(device, queue);
    let (emissive_texture, emissive_view) = make_emissive_placeholder(device, queue);
    LoadedTexture {
        diffuse_texture,
        diffuse_view,
        specular_texture,
        specular_view,
        normal_texture,
        normal_view,
        emissive_texture,
        emissive_view,
        mip_count: 1,
    }
}

fn d2_texture_slot_plan(
    header: &PrmHeader,
    slot_results: &[Result<PrmSlot, PrmReadError>; 4],
    policy: TextureSlotPolicy,
) -> Option<TextureSlotPlan> {
    (header.layer_count == 1).then(|| texture_slot_plan(header.slot_mask, slot_results, policy))
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

        let Some(plan) =
            d2_texture_slot_plan(&header, &slot_results, TextureSlotPolicy::WorldBundle)
        else {
            log::warn!(
                "[Loader] texture '{name}': .prm declares {} layers, but world textures require exactly 1 — using placeholders",
                header.layer_count,
            );
            out.push(placeholder_loaded_texture(device, queue));
            continue;
        };

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
        let (emissive_texture, emissive_view) = upload_slot_or_placeholder(
            device,
            queue,
            &slot_results[3],
            3,
            name,
            Slot::Emissive,
            plan.consume[3],
        );

        out.push(LoadedTexture {
            diffuse_texture,
            diffuse_view,
            specular_texture,
            specular_view,
            normal_texture,
            normal_view,
            emissive_texture,
            emissive_view,
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
    let Some(plan) =
        d2_texture_slot_plan(&header, &slot_results, TextureSlotPolicy::ModelDiffuseOnly)
    else {
        log::warn!(
            "[Loader] model texture '{name}': .prm declares {} layers, but model textures require exactly 1 — using placeholders",
            header.layer_count,
        );
        return placeholder_loaded_texture(device, queue);
    };
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
    let (emissive_texture, emissive_view) = make_emissive_placeholder(device, queue);

    LoadedTexture {
        diffuse_texture,
        diffuse_view,
        specular_texture,
        specular_view,
        normal_texture,
        normal_view,
        emissive_texture,
        emissive_view,
        mip_count: plan.mip_count,
    }
}

#[derive(Copy, Clone)]
enum Slot {
    Diffuse,
    Specular,
    Normal,
    Emissive,
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
            Slot::Emissive => make_emissive_placeholder(device, queue),
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
                Slot::Emissive => format!("Texture '{name}' Emissive"),
            };
            upload_texture_data(device, queue, format, &levels, &label)
        }
        Err(PrmReadError::NotPresent) => match slot {
            Slot::Diffuse => make_diffuse_placeholder(device, queue),
            Slot::Specular => make_specular_placeholder(device, queue),
            Slot::Normal => make_normal_placeholder(device, queue),
            Slot::Emissive => make_emissive_placeholder(device, queue),
        },
        Err(e) => {
            log::warn!(
                "[Loader] texture '{name}' slot {slot_idx}: .prm slot error: {e:?} — using placeholder"
            );
            match slot {
                Slot::Diffuse => make_diffuse_placeholder(device, queue),
                Slot::Specular => make_specular_placeholder(device, queue),
                Slot::Normal => make_normal_placeholder(device, queue),
                Slot::Emissive => make_emissive_placeholder(device, queue),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::prm::{PrmSlots, STAGE_VERSION};

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

    // Regression: parsed multi-layer PRMs reached the legacy one-chain mip
    // slicer, panicking in debug and uploading only layer 0 in release.
    #[test]
    fn d2_world_and_model_planning_rejects_multi_layer_prm() {
        let slot = PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width: 1,
            height: 1,
            level_count: 1,
            payload: vec![0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF],
        };
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE,
                bundle_hash: [0xA5; 32],
                total_body_bytes: 0,
                layer_count: 2,
            },
            slots: [Some(slot), None, None, None],
        };
        let bytes = file.to_bytes().expect("multi-layer fixture serializes");
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("multi-layer header parses");

        assert!(
            slots[0].is_ok(),
            "fixture must contain valid layer payloads"
        );
        assert!(
            d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::WorldBundle).is_none(),
            "world D2 upload must fall back before mip planning"
        );
        assert!(
            d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::ModelDiffuseOnly).is_none(),
            "model D2 upload must fall back before mip planning"
        );
    }

    // Regression: a hand-authored 2x2 BC5 normal declared zero mips, then the
    // D2 upload path indexed an empty level list as if mip 0 existed.
    #[test]
    fn d2_world_planning_degrades_zero_mip_bc5_normal_to_placeholder() {
        const HEADER_SIZE: usize = 45;
        const SLOT_HEADER_SIZE: usize = 12;
        let mut bytes = vec![0u8; HEADER_SIZE + SLOT_HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"PRM\x02");
        bytes[4] = STAGE_VERSION;
        bytes[5] = PrmSlots::NORMAL.bits();
        bytes[39..43].copy_from_slice(&(SLOT_HEADER_SIZE as u32).to_le_bytes());
        bytes[43..45].copy_from_slice(&1u16.to_le_bytes());
        bytes[HEADER_SIZE] = PrmFormat::Bc5RgUnorm as u8;
        bytes[HEADER_SIZE + 2..HEADER_SIZE + 4].copy_from_slice(&2u16.to_le_bytes());
        bytes[HEADER_SIZE + 4..HEADER_SIZE + 6].copy_from_slice(&2u16.to_le_bytes());
        // level_count and payload_bytes remain zero.

        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("slot failure must not poison the header");
        assert!(matches!(
            &slots[2],
            Err(PrmReadError::EmptyMipChain {
                slot: 2,
                format: PrmFormat::Bc5RgUnorm,
                width: 2,
                height: 2,
            })
        ));

        let plan = d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::WorldBundle)
            .expect("single-layer world bundle still produces a placeholder plan");
        assert!(
            plan.consume[2],
            "the slot error routes through placeholder upload"
        );
        assert_eq!(
            plan.mip_count, 1,
            "no valid mip chain disables mip filtering"
        );
    }
}
