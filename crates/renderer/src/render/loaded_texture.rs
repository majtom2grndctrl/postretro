// Loaded world and model material textures: the prepared-texture GPU pass
// and per-slot upload over the placeholder and upload primitives.
// See: context/lib/resource_management.md · context/lib/rendering_pipeline.md

mod placeholders;
mod upload;

use std::path::Path;
use std::time::{Duration, Instant};

use postretro_level_format::prm::{PrmReadError, PrmSlot};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_render_cpu::loaded_texture::slot_levels;
use postretro_render_cpu::surface_depth::{SurfaceReliefLevels, surface_relief_from_rg8_levels};
use postretro_render_cpu::texture_prep::{
    PreparedTexture, PreparedWorldTextures, prepare_world_textures,
};

pub(super) use placeholders::placeholder_loaded_texture;
use placeholders::{
    make_diffuse_placeholder, make_emissive_placeholder, make_normal_placeholder,
    make_specular_placeholder,
};
pub(super) use upload::prm_format_to_wgpu;
pub use upload::{upload_texture_array_data, upload_texture_data};

/// GPU resources for one world or model material.
/// World loading consumes all available slots; model loading consumes diffuse
/// only. Each uploaded slot carries its full mip chain.
pub struct LoadedTexture {
    pub diffuse_texture: wgpu::Texture,
    pub diffuse_view: wgpu::TextureView,
    /// Owned alongside `specular_view`; views borrow the texture, so dropping
    /// the texture invalidates the view. Shading samples through
    /// `specular_view`; the texture itself is read only for its METADATA, by
    /// `build_material_bind_group`, which decides Surface Depth's has-depth
    /// flag from the slot's format (`Rg8Unorm` = a two-channel surface map) and
    /// clamps the DDA's base mip to this slot's own uploaded chain.
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
    /// The specular slot's RAW relief band at every uploaded mip, measured from
    /// the bytes actually uploaded. [`SurfaceReliefLevels::FLAT`] for any slot
    /// that is not an `Rg8Unorm` surface map. The uniform build picks the base
    /// mip's band and quantizes it with the material's level count.
    pub surface_relief: SurfaceReliefLevels,
}

/// Where [`load_textures`] spent its time, for the install's own log line.
/// Log-only: nothing reads these to decide behavior.
#[derive(Debug, Default, Clone, Copy)]
pub struct TextureLoadTiming {
    /// `std::fs::read` of every `.prm` sidecar.
    pub read: Duration,
    /// `PrmFile::from_bytes_partial`: header decode and the per-slot payload copy.
    pub parse: Duration,
    /// Slot texture creation, the staged `write_texture` of every mip, and the
    /// surface-relief derivation. Placeholder fallbacks are not counted.
    pub create_write: Duration,
    /// Sidecar files read (a key shared by several names reads once per name).
    pub files_read: usize,
    /// Total bytes those reads returned.
    pub bytes_read: usize,
}

/// Load every world-material texture referenced by the PRL: the CPU pass
/// ([`prepare_world_textures`]) then the GPU pass ([`upload_prepared_textures`])
/// on this thread. Returns one `LoadedTexture` per entry, parallel to
/// `texture_names`, with the time spent reading, parsing and uploading them.
pub fn load_textures(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    texture_names: &[String],
    texture_cache_keys: &TextureCacheKeysSection,
    prm_cache_root: &Path,
) -> (Vec<LoadedTexture>, TextureLoadTiming) {
    let prepared = prepare_world_textures(texture_names, texture_cache_keys, prm_cache_root);
    upload_prepared_textures(device, queue, &prepared)
}

/// The GPU pass over prepared world materials: creates every texture and stages
/// every mip write on `queue`. Runs on whichever thread holds the device and
/// queue; the writes reach the GPU at that queue's next submit.
pub fn upload_prepared_textures(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    prepared: &PreparedWorldTextures,
) -> (Vec<LoadedTexture>, TextureLoadTiming) {
    let mut timing = TextureLoadTiming {
        read: prepared.timing.read,
        parse: prepared.timing.parse,
        create_write: Duration::ZERO,
        files_read: prepared.timing.files_read,
        bytes_read: prepared.timing.bytes_read,
    };
    let out = prepared
        .entries
        .iter()
        .map(|entry| {
            if matches!(entry, PreparedTexture::Placeholder) {
                return placeholder_loaded_texture(device, queue);
            }
            let create_started = Instant::now();
            let texture = upload_prepared_texture(device, queue, entry);
            timing.create_write += create_started.elapsed();
            texture
        })
        .collect();
    (out, timing)
}

/// The GPU half of one prepared material, world or model: the plan's policy
/// already decided which slots it consumes, and every other slot binds its
/// placeholder.
pub(super) fn upload_prepared_texture(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    prepared: &PreparedTexture,
) -> LoadedTexture {
    let PreparedTexture::Bundle { name, slots, plan } = prepared else {
        return placeholder_loaded_texture(device, queue);
    };
    let (diffuse_texture, diffuse_view) = upload_slot_or_placeholder(
        device,
        queue,
        &slots[0],
        0,
        name,
        Slot::Diffuse,
        plan.consume[0],
    );
    let (specular_texture, specular_view) = upload_slot_or_placeholder(
        device,
        queue,
        &slots[1],
        1,
        name,
        Slot::Specular,
        plan.consume[1],
    );
    let (normal_texture, normal_view) = upload_slot_or_placeholder(
        device,
        queue,
        &slots[2],
        2,
        name,
        Slot::Normal,
        plan.consume[2],
    );
    let (emissive_texture, emissive_view) = upload_slot_or_placeholder(
        device,
        queue,
        &slots[3],
        3,
        name,
        Slot::Emissive,
        plan.consume[3],
    );

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
        // A model plan never consumes specular, so models stay flat.
        surface_relief: specular_surface_relief(&slots[1], plan.consume[1]),
    }
}

/// The raw relief band of every mip of the specular slot, from the exact bytes
/// `upload_slot_or_placeholder` uploads for it.
///
/// Empty ([`SurfaceReliefLevels::FLAT`]) unless the slot is consumed AND decodes as
/// an `Rg8Unorm` surface map — the same condition under which the bound
/// texture has that format. Any other slot must never report relief: an R8
/// slot's G would read as maximum raise under the signed encoding.
fn specular_surface_relief(
    slot_result: &Result<PrmSlot, PrmReadError>,
    consume: bool,
) -> SurfaceReliefLevels {
    match slot_result {
        Ok(slot) if consume && prm_format_to_wgpu(slot.format) == wgpu::TextureFormat::Rg8Unorm => {
            surface_relief_from_rg8_levels(&slot_levels(slot))
        }
        _ => SurfaceReliefLevels::FLAT,
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
    queue: &dyn crate::render::queue_writes::QueueWrites,
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
    use postretro_level_format::prm::PrmFormat;

    fn slot(format: PrmFormat, width: u16, height: u16, payload: Vec<u8>) -> PrmSlot {
        PrmSlot {
            format,
            width,
            height,
            level_count: 1,
            payload,
        }
    }

    /// The relief is measured from the bytes uploaded, in the slot's own
    /// chain: a stored G of 0 is authored white (max raise), 255 is black.
    #[test]
    fn specular_relief_is_measured_from_the_uploaded_surface_map() {
        // Two texels of [R, G]: authored 128 (stored 127) and authored 192
        // (stored 63).
        let slot = slot(PrmFormat::Rg8Unorm, 2, 1, vec![9, 127, 9, 63]);
        let relief = specular_surface_relief(&Ok(slot), true).at(0);
        assert_eq!(relief.peak_raise, 0.5);
        assert_eq!(relief.trough, 0.0);
    }

    /// A single-channel or unconsumed specular slot has no relief, even when
    /// its bytes would read as maximum raise (stored G = 0).
    #[test]
    fn a_non_surface_map_slot_reports_no_relief() {
        let r8 = slot(PrmFormat::R8Unorm, 2, 1, vec![0, 0]);
        assert_eq!(
            specular_surface_relief(&Ok(r8), true),
            SurfaceReliefLevels::FLAT
        );
        let unconsumed = slot(PrmFormat::Rg8Unorm, 1, 1, vec![0, 0]);
        assert_eq!(
            specular_surface_relief(&Ok(unconsumed), false),
            SurfaceReliefLevels::FLAT
        );
        assert_eq!(
            specular_surface_relief(&Err(PrmReadError::NotPresent), true),
            SurfaceReliefLevels::FLAT
        );
    }
}
