// Renderer world-texture install: main-thread load, off-main upload, and the
// one bind-group path both share.
// See: context/lib/boot_sequence.md §2–§3 · context/lib/resource_management.md

use super::loaded_texture::upload_prepared_textures;
use super::renderer_types::FullRenderer;
use super::*;
use postretro_render_cpu::texture_prep::PreparedWorldTextures;

/// The GPU pass's product for one load's world materials, created on the
/// upload thread and bound by nothing until [`Renderer::install_uploaded_textures`].
pub struct UploadedWorldTextures {
    pub(crate) textures: Vec<LoadedTexture>,
    pub(crate) timing: TextureLoadTiming,
}

impl Renderer {
    /// Builds one load's material bind groups from baked `.prm` mip sidecars
    /// and stages them for the next `install_level_geometry`, which takes
    /// them into the level set it publishes.
    /// `texture_materials` must be parallel to `texture_names`; entries beyond
    /// its length fall back to `Material::Default`. Caller drives the order:
    /// `install_textures` runs before `install_level_geometry` because the
    /// uploaded diffuse dimensions feed `normalize_world_uvs`.
    /// See: context/lib/boot_sequence.md §3 (Level Install Order) · context/lib/build_pipeline.md
    pub fn install_textures(
        &mut self,
        texture_names: &[String],
        texture_cache_keys: &TextureCacheKeysSection,
        prm_cache_root: &Path,
        texture_materials: &[Material],
    ) {
        let install_started = std::time::Instant::now();
        let surface_depth_quality = self.surface_depth_quality;
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let _installation = queue.installation();
        let queue = queue.raw();
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");

        let (loaded, load_timing) = load_textures(
            device,
            queue,
            texture_names,
            texture_cache_keys,
            prm_cache_root,
        );
        bind_loaded_textures(
            device,
            queue,
            full,
            surface_depth_quality,
            loaded,
            &load_timing,
            texture_materials,
            install_started.elapsed(),
        );
    }

    /// Runs the GPU pass for one load's prepared world materials on the upload
    /// thread. Poll the handle; nothing it creates is bound until
    /// [`Self::install_uploaded_textures`].
    pub fn begin_world_texture_upload(
        &self,
        prepared: PreparedWorldTextures,
    ) -> PendingUpload<UploadedWorldTextures> {
        let upload_thread: &UploadThread = &self.full().upload_thread;
        upload_thread.submit(move |context: &UploadContext| {
            let (textures, timing) =
                upload_prepared_textures(&context.device, &context.queue, &prepared);
            Ok(UploadedWorldTextures { textures, timing })
        })
    }

    /// Binds world materials the upload thread created: the same sampler-pool,
    /// bind-group and placeholder-fallback path as [`Self::install_textures`].
    pub fn install_uploaded_textures(
        &mut self,
        uploaded: UploadedWorldTextures,
        texture_materials: &[Material],
    ) {
        let surface_depth_quality = self.surface_depth_quality;
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let _installation = queue.installation();
        let queue = queue.raw();
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let UploadedWorldTextures { textures, timing } = uploaded;
        // Off-main reads, parses and uploads run before this frame; they are
        // the whole pre-bind share of the install.
        let before_bind_groups = timing.read + timing.parse + timing.create_write;
        bind_loaded_textures(
            device,
            queue,
            full,
            surface_depth_quality,
            textures,
            &timing,
            texture_materials,
            before_bind_groups,
        );
    }
}

/// Stages `loaded` as the next level's materials: one sampler per new mip
/// count, one bind group per texture, or the placeholder slot when the level
/// has none. `before_bind_groups` is the install time already spent, for the
/// log line.
#[allow(clippy::too_many_arguments)]
fn bind_loaded_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    full: &mut FullRenderer,
    surface_depth_quality: postretro_render_cpu::surface_depth::SurfaceDepthQuality,
    loaded: Vec<LoadedTexture>,
    load_timing: &TextureLoadTiming,
    texture_materials: &[Material],
    before_bind_groups: std::time::Duration,
) {
    let bind_groups_started = std::time::Instant::now();
    let total = || before_bind_groups + bind_groups_started.elapsed();

    // Every distinct `mip_count` seen in this batch needs a sampler with
    // matching `lod_max_clamp`. The renderer's add-only map serves the ones it
    // holds (the `1` entry seeded in `Renderer::new` covers placeholders); the
    // rest are created into the staged materials' own map, which merges into
    // the renderer's when their set publishes.
    let mut samplers = SamplerMaps::default();
    let mut gpu_textures: Vec<GpuTexture> = Vec::with_capacity(loaded.len());
    for (idx, tex) in loaded.iter().enumerate() {
        let aniso_sampler = samplers.aniso(&full.samplers, device, tex.mip_count);
        let material = texture_materials
            .get(idx)
            .copied()
            .unwrap_or(postretro_render_data::material::Material::Default);
        let binding = build_material_bind_group(
            device,
            &full.texture_bind_group_layout,
            tex,
            &aniso_sampler,
            material,
            surface_depth_quality,
            &format!("Material {idx}"),
        );
        gpu_textures.push(binding.into());
    }

    if gpu_textures.is_empty() {
        // No textures referenced by the level — keep the placeholder slot
        // so the world pipeline still has a bind group bound.
        let mut textures = placeholder_level_textures(device, queue, full, surface_depth_quality);
        textures.stored_texture_materials = texture_materials.to_vec();
        full.pending_textures = Some(textures);
        log::info!("[Renderer] Textures installed: 1 (placeholder fallback)");
        log_texture_install_timing(load_timing, bind_groups_started.elapsed(), total());
        return;
    }

    log::info!("[Renderer] Textures installed: {}", gpu_textures.len());
    full.pending_textures = Some(LevelTextures {
        loaded_textures: loaded,
        gpu_textures,
        stored_texture_materials: texture_materials.to_vec(),
        samplers,
    });
    log_texture_install_timing(load_timing, bind_groups_started.elapsed(), total());
}

/// One placeholder material slot, for a level that references no texture or
/// a geometry install with no staged or kept materials.
pub(super) fn placeholder_level_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    full: &FullRenderer,
    surface_depth_quality: postretro_render_cpu::surface_depth::SurfaceDepthQuality,
) -> LevelTextures {
    let placeholder = placeholder_loaded_texture(device, queue);
    let aniso_sampler = full
        .samplers
        .aniso
        .get(&1)
        .expect("mip_count 1 aniso sampler is seeded at Renderer::new");
    let binding = build_material_bind_group(
        device,
        &full.texture_bind_group_layout,
        &placeholder,
        aniso_sampler,
        postretro_render_data::material::Material::Default,
        surface_depth_quality,
        "Placeholder Material",
    );
    LevelTextures {
        loaded_textures: vec![placeholder],
        gpu_textures: vec![binding.into()],
        stored_texture_materials: Vec::new(),
        samplers: SamplerMaps::default(),
    }
}

/// Log-only phase split of the `texture_upload` stage, in the renderer's own
/// line because it never sees app-side timing types (same shape as
/// `[Renderer] Geometry install timing:`). `bind_groups` covers the sampler pool
/// and one material bind group per texture, after the files are read, parsed
/// and uploaded; `other` is the remainder of `total`.
fn log_texture_install_timing(
    load: &TextureLoadTiming,
    bind_groups: std::time::Duration,
    total: std::time::Duration,
) {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let other = total.saturating_sub(load.read + load.parse + load.create_write + bind_groups);
    log::info!(
        "[Renderer] Texture install timing: total={:.1}ms, read={:.1}ms, parse={:.1}ms, \
         create_write={:.1}ms, bind_groups={:.1}ms, other={:.1}ms \
         ({} sidecar read(s), {:.1} MiB)",
        ms(total),
        ms(load.read),
        ms(load.parse),
        ms(load.create_write),
        ms(bind_groups),
        ms(other),
        load.files_read,
        load.bytes_read as f64 / (1024.0 * 1024.0),
    );
}

#[cfg(test)]
#[path = "renderer_textures_tests.rs"]
pub(super) mod tests;
