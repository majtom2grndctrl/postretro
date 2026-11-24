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
use postretro_render_cpu::surface_depth::{SurfaceReliefLevels, surface_relief_from_surface_map};
use postretro_render_cpu::texture_prep::{
    PreparedTexture, PreparedWorldTextures, prepare_world_textures,
};

use crate::render::specular_surface_map_bytes_per_texel;

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
    /// flag from the slot's format (`Rg8Unorm` or `Rgba8Unorm` = a surface map)
    /// and clamps the DDA's base mip to this slot's own uploaded chain.
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
    /// that is not a surface map. The uniform build picks the base
    /// mip's band and quantizes it with the material's level count.
    pub surface_relief: SurfaceReliefLevels,
    /// Whether the material reflects its room: the glossy opt-in over the
    /// specular slot's uploaded mip 0, the bytes the compiler judged
    /// (`specular_slot_reflects`). Its bucket is then a glossy bucket.
    pub reflects: bool,
    /// Whether the normal slot holds the material's own normal map rather
    /// than the 1×1 neutral placeholder: the glossy overlay reads a
    /// placeholder's one texel instead of sampling it.
    pub normal_mapped: bool,
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
        reflects: specular_slot_reflects_in(&slots[1], plan.consume[1]),
        normal_mapped: plan.consume[2] && slots[2].is_ok(),
    }
}

/// The glossy opt-in over the consumed specular slot's mip 0, the level
/// `upload_slot_or_placeholder` uploads first. A slot the plan does not
/// consume, or one that failed to read, binds a placeholder, which never
/// reflects.
fn specular_slot_reflects_in(slot_result: &Result<PrmSlot, PrmReadError>, consume: bool) -> bool {
    let Ok(slot) = slot_result else {
        return false;
    };
    consume
        && slot_levels(slot).first().is_some_and(|&(_, _, mip0)| {
            postretro_level_format::glossy_opt_in::specular_slot_reflects(slot.format, mip0)
        })
}

/// The raw relief band of every mip of the specular slot, from the exact bytes
/// `upload_slot_or_placeholder` uploads for it.
///
/// Empty ([`SurfaceReliefLevels::FLAT`]) unless the slot is consumed AND decodes as
/// a two- or four-channel surface map — the same format test that sets the
/// bound texture's has-depth flag, read at that format's stride. Any other slot
/// must never report relief: an R8 slot's G would read as maximum raise under
/// the signed encoding.
fn specular_surface_relief(
    slot_result: &Result<PrmSlot, PrmReadError>,
    consume: bool,
) -> SurfaceReliefLevels {
    let Ok(slot) = slot_result else {
        return SurfaceReliefLevels::FLAT;
    };
    match specular_surface_map_bytes_per_texel(prm_format_to_wgpu(slot.format)) {
        Some(bytes_per_texel) if consume => {
            surface_relief_from_surface_map(&slot_levels(slot), bytes_per_texel)
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

    /// Edge of the multi-mip specular fixtures: 4×4, 2×2 and 1×1 levels.
    const EDGE: u16 = 4;

    /// Stored height per texel and mip, as the bake writes it (`255 − h`). The
    /// coarser mips overshoot the base level's band, as the bake's filter can.
    fn stored_height(level: usize, texel: usize) -> u8 {
        match level {
            0 => [127, 127, 200, 64, 90, 127, 180, 127][texel % 8],
            1 => [40, 127, 220, 127][texel],
            _ => 10,
        }
    }

    /// One four-channel texel as the bake writes it: R specular, G stored
    /// height (`height` false stores the plane, 127, at every mip), B
    /// smoothness held to 1..=255, A tint held to 0..=254. B and A never equal
    /// G, so a misread channel shows.
    fn rgba_texel(level: usize, texel: usize, height: bool) -> [u8; 4] {
        let g = if height {
            stored_height(level, texel)
        } else {
            127
        };
        let b = (g ^ 0x5A).max(1);
        let a = (g.wrapping_add(31)).min(254);
        [0x80, g, b, a]
    }

    /// A 4×4 specular slot of `channels` bytes per texel, three mips,
    /// serialized as a `.prm` and read back through the runtime reader.
    fn loaded_specular_slot(channels: usize, height: bool) -> Result<PrmSlot, PrmReadError> {
        use postretro_level_format::prm::{
            PrmFile, PrmHeader, PrmSlots, STAGE_VERSION, expected_level_count,
        };
        let format = match channels {
            1 => PrmFormat::R8Unorm,
            2 => PrmFormat::Rg8Unorm,
            4 => PrmFormat::Rgba8Unorm,
            other => panic!("no {other}-channel specular slot"),
        };
        let level_count = expected_level_count(EDGE, EDGE);
        let mut payload = Vec::new();
        for level in 0..usize::from(level_count) {
            let texels = (usize::from(EDGE) >> level).pow(2);
            for texel in 0..texels {
                payload.extend_from_slice(&rgba_texel(level, texel, height)[..channels]);
            }
        }
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::SPECULAR,
                bundle_hash: [0u8; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [
                None,
                Some(PrmSlot {
                    format,
                    width: EDGE,
                    height: EDGE,
                    level_count,
                    payload,
                }),
                None,
                None,
            ],
        };
        let bytes = file.to_bytes().expect("the specular slot serializes");
        let (header, [_, specular, _, _]) = PrmFile::from_bytes_partial(&bytes);
        header.expect("the header parses");
        specular
    }

    /// The uniform plan `build_material_bind_group` packs for `slot` under
    /// `material`, from the same three facts it reads off the loaded slot.
    fn plan_for(
        material: postretro_render_data::material::Material,
        slot: &Result<PrmSlot, PrmReadError>,
    ) -> postretro_render_cpu::material_plan::MaterialUniformPlan {
        let (format, mip_count) = match slot {
            Ok(slot) => (prm_format_to_wgpu(slot.format), u32::from(slot.level_count)),
            // The 1×1 single-channel placeholder an absent slot binds.
            Err(_) => (wgpu::TextureFormat::R8Unorm, 1),
        };
        crate::render::material_uniform_plan(
            postretro_render_data::material::TextureMaterial {
                material,
                emissive_strength: 2.0,
            },
            format,
            mip_count,
            specular_surface_relief(slot, true),
        )
    }

    /// With `_h`, a four-channel slot loaded through the runtime
    /// reader sets has-depth, and at every uploaded mip its relief band equals
    /// the two-channel band from the same height. Its sharpness and tint bytes
    /// differ from the height bytes, so reading B or A as height would move it.
    #[test]
    fn four_channel_slot_with_height_sets_has_depth_and_measures_the_two_channel_band() {
        let rgba = loaded_specular_slot(4, true);
        let rg = loaded_specular_slot(2, true);
        let rgba_slot = rgba.as_ref().expect("slot 1 admits Rgba8Unorm");
        assert_eq!(rgba_slot.format, PrmFormat::Rgba8Unorm);
        assert!(crate::render::specular_slot_is_surface_map(
            prm_format_to_wgpu(rgba_slot.format)
        ));
        for texel in rgba_slot.payload.as_chunks::<4>().0 {
            assert_ne!(texel[2], texel[1], "fixture: sharpness differs from height");
            assert_ne!(texel[3], texel[1], "fixture: tint differs from height");
        }

        let rgba_relief = specular_surface_relief(&rgba, true);
        let rg_relief = specular_surface_relief(&rg, true);
        for level in 0..u32::from(rgba_slot.level_count) {
            assert_eq!(rgba_relief.at(level), rg_relief.at(level), "mip {level}");
            assert!(
                !rgba_relief.at(level).is_flat(),
                "mip {level} carries relief"
            );
        }
        assert_ne!(
            rgba_relief.at(0),
            rgba_relief.at(1),
            "each mip keeps its own band"
        );

        // The plan the bind group packs agrees: has-depth set, and at Surface
        // Depth On the same bytes as the two-channel slot.
        use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
        let material = postretro_render_data::material::Material::Concrete;
        let (four, two) = (plan_for(material, &rgba), plan_for(material, &rg));
        assert!(four.specular_is_surface_map);
        assert!(
            four.surface_depth_uniform(SurfaceDepthQuality::On)
                .has_depth
        );
        for quality in SurfaceDepthQuality::ALL {
            assert_eq!(
                four.uniform_bytes(quality),
                two.uniform_bytes(quality),
                "{quality:?}"
            );
        }
    }

    /// Without `_h`, a four-channel slot on a prefix with
    /// Surface Depth tuning sets has-depth, yet at Surface Depth On (and Off)
    /// uploads the relief row byte-equal to the same prefix with no height
    /// map: a single-channel `_s` slot, or no slot at all (the placeholder).
    /// The stored plane (G = 127 at every mip) measures as an empty band.
    #[test]
    fn four_channel_slot_without_height_uploads_the_no_height_relief_row() {
        use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
        let material = postretro_render_data::material::Material::Concrete;
        assert!(
            material.surface_depth().is_enabled(),
            "the prefix must carry Surface Depth tuning, or the row is vacuous"
        );
        let four = loaded_specular_slot(4, false);
        let plan = plan_for(material, &four);
        assert!(
            plan.specular_is_surface_map,
            "has-depth comes from the format"
        );
        for level in 0..3 {
            assert!(plan.surface_relief.at(level).is_flat(), "mip {level}");
        }
        for no_height in [
            loaded_specular_slot(1, false),
            Err(PrmReadError::NotPresent),
        ] {
            let no_height = plan_for(material, &no_height);
            assert!(!no_height.specular_is_surface_map);
            for quality in SurfaceDepthQuality::ALL {
                assert_eq!(
                    plan.uniform_bytes(quality),
                    no_height.uniform_bytes(quality),
                    "{quality:?}"
                );
            }
        }
        // The same slot with `_h` does march, so the equality above is the
        // empty band's doing, not an unopened gate.
        let with_height = plan_for(material, &loaded_specular_slot(4, true));
        assert!(
            with_height
                .surface_depth_uniform(SurfaceDepthQuality::On)
                .has_depth
        );
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
        for unconsumed in [
            slot(PrmFormat::Rg8Unorm, 1, 1, vec![0, 0]),
            slot(PrmFormat::Rgba8Unorm, 1, 1, vec![0, 0, 0, 0]),
        ] {
            assert_eq!(
                specular_surface_relief(&Ok(unconsumed), false),
                SurfaceReliefLevels::FLAT
            );
        }
        assert_eq!(
            specular_surface_relief(&Err(PrmReadError::NotPresent), true),
            SurfaceReliefLevels::FLAT
        );
    }

    /// Reads every texel of every mip of a specular view twice: `textureLoad`,
    /// and `textureSampleLevel` at the texel centre through the material
    /// sampler. Two `vec4<f32>` per probe, load first.
    const SPECULAR_READBACK_WGSL: &str = r#"
@group(0) @binding(0) var slot_texture: texture_2d<f32>;
@group(0) @binding(1) var slot_sampler: sampler;
@group(0) @binding(2) var<storage, read> probes: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> texels: array<vec4<f32>>;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let probe = probes[id.x];
    let level = i32(probe.z);
    let dims = vec2<f32>(textureDimensions(slot_texture, level));
    let uv = (vec2<f32>(probe.xy) + 0.5) / dims;
    texels[2u * id.x] = textureLoad(slot_texture, vec2<i32>(probe.xy), level);
    texels[2u * id.x + 1u] = textureSampleLevel(slot_texture, slot_sampler, uv, f32(level));
}
"#;

    /// `(load, sample)` per texel of every mip of `view`, in mip then
    /// row-major order.
    fn read_specular_texels(
        ctx: &crate::render::gpu_test_harness::GpuCtx,
        texture: &wgpu::Texture,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> Vec<([f32; 4], [f32; 4])> {
        use wgpu::util::DeviceExt;
        let device = &ctx.device;
        let mut probes: Vec<u32> = Vec::new();
        for level in 0..texture.mip_level_count() {
            let (w, h) = (
                (texture.width() >> level).max(1),
                (texture.height() >> level).max(1),
            );
            for y in 0..h {
                for x in 0..w {
                    probes.extend([x, y, level, 0]);
                }
            }
        }
        let probe_count = probes.len() / 4;
        let probe_bytes: Vec<u8> = probes.iter().flat_map(|v| v.to_le_bytes()).collect();
        let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("specular readback probes"),
            contents: &probe_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let out_size = (probe_count * 2 * 16) as u64;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("specular readback texels"),
            size: out_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("specular readback map"),
            size: out_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let compute = wgpu::ShaderStages::COMPUTE;
        let storage = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("specular readback layout"),
            entries: &[
                // The material layout's binding 2 and binding 5 types.
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(2, storage(true)),
                entry(3, storage(false)),
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("specular readback bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: probe_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("specular readback shader"),
            source: wgpu::ShaderSource::Wgsl(SPECULAR_READBACK_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("specular readback pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("specular readback pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(probe_count as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out, 0, &readback, 0, out_size);
        ctx.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).ok();
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        rx.recv().expect("map channel").expect("map ok");
        let data = slice.get_mapped_range().expect("readback mapped");
        let floats: Vec<f32> = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        drop(data);
        readback.unmap();
        floats
            .as_chunks::<8>()
            .0
            .iter()
            .map(|pair| {
                (
                    [pair[0], pair[1], pair[2], pair[3]],
                    [pair[4], pair[5], pair[6], pair[7]],
                )
            })
            .collect()
    }

    /// The specular slot uploads through the real load path in every width
    /// the bake writes, binds at the material layout's binding 2 beside the
    /// binding-5 aniso sampler, and reads back as the shader will see it:
    /// every stored channel is its byte over 255, and a narrower slot (and the
    /// placeholder) reads a missing G and B as 0 and a missing A as 1 — the
    /// "no smoothness" and "no tint" markers. The bind group's uniform plan
    /// carries has-depth from the format and packs rows 32 and 33's bytes.
    #[test]
    fn uploaded_specular_slots_read_their_stored_channels_and_widen_narrow_ones_to_the_markers() {
        use crate::render::gpu_test_harness::gpu_or_skip;
        use postretro_render_cpu::loaded_texture::{TextureSlotPlan, header_mip_count};
        use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
        use postretro_render_cpu::texture_prep::PreparedTexture;
        let test = "uploaded_specular_slots_read_their_stored_channels_and_widen_narrow_ones_to_the_markers";
        let Some(ctx) = gpu_or_skip(test) else {
            return;
        };
        let device = &ctx.device;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
            entries: &crate::render::pipeline_layout::material_bind_group_layout_entries(),
        });
        let material = postretro_render_data::material::TextureMaterial {
            material: postretro_render_data::material::Material::Concrete,
            emissive_strength: 2.0,
        };

        // (case, channels stored, `_h`, has-depth, expected wgpu format);
        // channels 0 is the absent slot, which binds the placeholder.
        let cases = [
            ("_s", 1, false, false, wgpu::TextureFormat::R8Unorm),
            ("_s+_h", 2, true, true, wgpu::TextureFormat::Rg8Unorm),
            ("_p/_t+_h", 4, true, true, wgpu::TextureFormat::Rgba8Unorm),
            ("_p/_t", 4, false, true, wgpu::TextureFormat::Rgba8Unorm),
            ("absent", 0, false, false, wgpu::TextureFormat::R8Unorm),
        ];
        let mut uniforms = Vec::new();
        for (case, channels, height, has_depth, format) in cases {
            let specular = match channels {
                0 => Err(PrmReadError::NotPresent),
                channels => loaded_specular_slot(channels, height),
            };
            let slots = [
                Err(PrmReadError::NotPresent),
                specular,
                Err(PrmReadError::NotPresent),
                Err(PrmReadError::NotPresent),
            ];
            let plan = TextureSlotPlan {
                consume: [true; 4],
                mip_count: header_mip_count(&slots),
            };
            let prepared = PreparedTexture::Bundle {
                name: format!("test/{case}"),
                slots,
                plan,
            };
            let loaded = upload_prepared_texture(device, &ctx.queue, &prepared);
            assert_eq!(loaded.specular_texture.format(), format, "{case}");
            let sampler = crate::render::create_mip_aniso_sampler(device, loaded.mip_count);
            let binding = crate::render::build_material_bind_group(
                device,
                &material_layout,
                &loaded,
                &sampler,
                material,
                SurfaceDepthQuality::On,
                case,
            );
            assert_eq!(
                binding.uniform_plan.specular_is_surface_map, has_depth,
                "{case}"
            );
            uniforms.push(binding.uniform_plan.uniform_bytes(SurfaceDepthQuality::On));

            let read = read_specular_texels(
                &ctx,
                &loaded.specular_texture,
                &loaded.specular_view,
                &sampler,
            );
            let mut index = 0;
            for level in 0..loaded.specular_texture.mip_level_count() as usize {
                let texels = (loaded.specular_texture.width() as usize >> level)
                    .max(1)
                    .pow(2);
                for texel in 0..texels {
                    let stored = if channels == 0 {
                        [0, 0, 0, 0]
                    } else {
                        rgba_texel(level, texel, height)
                    };
                    let expected: [f32; 4] =
                        std::array::from_fn(|channel| match (channel < channels.max(1), channel) {
                            (true, _) => f32::from(stored[channel]) / 255.0,
                            (false, 3) => 1.0,
                            (false, _) => 0.0,
                        });
                    let (load, sample) = read[index];
                    index += 1;
                    for channel in 0..4 {
                        let what = format!("{case}: mip {level} texel {texel} channel {channel}");
                        if channel >= channels.max(1) {
                            // A marker: exact, through the load and the
                            // filtered sample alike.
                            assert_eq!(load[channel], expected[channel], "{what} load");
                            assert_eq!(sample[channel], expected[channel], "{what} sample");
                        } else {
                            assert!(
                                (load[channel] - expected[channel]).abs() <= 1.0e-6,
                                "{what} load: {load:?} vs {expected:?}"
                            );
                            assert!(
                                (sample[channel] - expected[channel]).abs() <= 0.5 / 255.0,
                                "{what} sample: {sample:?} vs {expected:?}"
                            );
                        }
                    }
                }
            }
            assert_eq!(index, read.len(), "{case}: every texel was checked");
        }
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "upload or bind raised {error:?}");

        // With `_h`, through the real bind group: a four-channel slot
        // packs the two-channel slot's bytes. Without `_h`, the
        // single-channel slot's and the placeholder's.
        assert_eq!(uniforms[2], uniforms[1]);
        assert_eq!(uniforms[3], uniforms[0]);
        assert_eq!(uniforms[3], uniforms[4]);
        assert_ne!(uniforms[1], uniforms[0], "the `_h` slot marches");
    }
}
