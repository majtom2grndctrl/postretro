// Per-frame renderer plumbing: surface resize, per-frame uniform updates, and
// debug-line clearing.
// See: context/lib/rendering_pipeline.md §1

use super::*;

/// First-person weapon projection. Kept renderer-owned because this is a GPU
/// pass contract; game assembly supplies a world-space model transform plus the
/// matching render-camera view and never needs a wgpu type.
const VIEWMODEL_HFOV_RADIANS: f32 = 70.0_f32.to_radians();
const VIEWMODEL_NEAR_CLIP: f32 = 0.01;
const VIEWMODEL_FAR_CLIP: f32 = 2.0;

fn viewmodel_projection(aspect: f32) -> Mat4 {
    let safe_aspect = aspect.max(0.1);
    let vertical_fov = 2.0 * ((VIEWMODEL_HFOV_RADIANS / 2.0).tan() / safe_aspect).atan();
    Mat4::perspective_rh(
        vertical_fov,
        safe_aspect,
        VIEWMODEL_NEAR_CLIP,
        VIEWMODEL_FAR_CLIP,
    )
}

impl Renderer {
    /// Submit a texture-to-buffer copy, wait for its completion, and return a
    /// tight RGBA8 grid. wgpu requires rows in copies to be 256-byte aligned;
    /// the caller receives the padding-free pixels the capture contract exposes.
    pub(super) fn read_texture_rgba8(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        mut encoder: wgpu::CommandEncoder,
    ) -> Result<Vec<u8>> {
        let unpadded_bytes_per_row = width
            .checked_mul(4)
            .context("capture row byte count overflows u32")?;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            .checked_mul(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            .context("capture padded row byte count overflows u32")?;
        let buffer_size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("capture readback buffer size overflows u64")?;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Frame Capture Readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("waiting for capture readback failed")?;
        rx.recv()
            .context("capture readback map callback did not complete")?
            .context("capture readback map failed")?;

        let data = slice.get_mapped_range();
        let tight_len = u64::from(unpadded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("capture tight byte count overflows u64")?;
        let tight_capacity = usize::try_from(tight_len)
            .context("capture tight byte count exceeds addressable memory")?;
        let mut tight = Vec::with_capacity(tight_capacity);
        for row in 0..height {
            let start = usize::try_from(u64::from(row) * u64::from(padded_bytes_per_row))
                .context("capture row offset exceeds addressable memory")?;
            let end = start
                .checked_add(usize::try_from(unpadded_bytes_per_row)?)
                .context("capture row end exceeds addressable memory")?;
            tight.extend_from_slice(&data[start..end]);
        }
        drop(data);
        buffer.unmap();

        Ok(tight)
    }

    pub(super) fn reconfigure_surface(&mut self) {
        self.is_surface_configured = false;
        self.surface
            .as_ref()
            .expect("surface reconfigure requires a windowed renderer")
            .configure(&self.device, &self.surface_config);
        self.is_surface_configured = true;
        self.surface_reconfigure_pending = false;
    }

    pub(super) fn acquire_present_handle(&mut self, phase: &str) -> Result<Option<PresentHandle>> {
        if self.surface.is_none() {
            anyhow::bail!("{phase} requires a windowed renderer");
        }
        if self.surface_reconfigure_pending {
            self.reconfigure_surface();
        }

        let output = match self
            .surface
            .as_ref()
            .expect("surface presence checked above")
            .get_current_texture()
        {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface_reconfigure_pending = true;
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure_surface();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("[Renderer] surface lost during {phase}; reconfiguring");
                self.reconfigure_surface();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error during {phase}");
            }
        };

        Ok(Some(PresentHandle::new(output)))
    }

    /// Camera owns aspect ratio; caller must also call `update_per_frame_uniforms`.
    ///
    /// Works in BOTH windowed phases. Windowed renderers reconfigure the surface
    /// during boot; full-phase depth, HDR scene/bloom, fog, SDF shadow, and
    /// spot-shadow resources rebuild only when the full renderer exists.
    /// Offscreen renderers have no surface, so resize is a no-op.
    /// During the boot/splash window (`full` is `None`) the surface is the only
    /// thing that needs resizing — the boot splash re-projects against the new
    /// backbuffer size on the next `render_splash_frame`.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let Some(surface) = self.surface.as_ref() else {
            return;
        };
        self.surface_config.width = width;
        self.surface_config.height = height;
        surface.configure(&self.device, &self.surface_config);
        self.is_surface_configured = true;
        self.surface_reconfigure_pending = false;

        // Full-phase targets only — skip when boot-only (splash still presents).
        if self.full.is_none() {
            return;
        }
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("full renderer present (checked above)");
        let (_depth_texture, depth_view) = create_depth_texture(device, width, height);
        full.depth_view = depth_view;
        // Recreate the surface-sized HDR scene target before bloom so bloom can
        // rebuild its source view and resolution-dependent parameter table.
        full.screen_effects.resize(device, width, height);
        full.bloom.resize(
            device,
            width,
            height,
            full.screen_effects.scene_color_texture(),
        );
        full.fog.resize(device, width, height, &full.depth_view);
        // SDF shadow target is half-res relative to the surface; the depth view
        // also changed, so the pass bind group has to be rebuilt.
        full.sdf_shadow_pass
            .resize(device, &full.depth_view, width, height);
        // Group-5 bind group references both the SDF shadow factor target
        // and the scene depth — both just got recreated, so rebuild. The cube
        // binding's presence is fixed for the renderer's lifetime: the pool is
        // `Some` iff the adapter supports CUBE_ARRAY_TEXTURES, so rebuild the BGL
        // with the same flag (its presence mirrors the pool's).
        let cube_array_supported = full.cube_shadow_pool.is_some();
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(device, cube_array_supported);
        // The cube sampling view is surface-size-independent, but the group-5
        // bind group is fully rebuilt here, so re-reference it (`Some` when the
        // pool is present, `None` omits binding 5 to match the BGL).
        let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        full.spot_shadow_pool.rebuild_bind_group(
            device,
            &spot_shadow_bgl,
            &full.sdf_shadow_pass.shadow_view,
            &full.depth_view,
            cube_sampling_view,
        );
    }

    pub fn update_per_frame_uniforms(
        &mut self,
        view_proj: Mat4,
        camera_position: Vec3,
        script_time: f32,
    ) {
        // Animation clock is the level-relative `script_time` (the same clock
        // the light bridge evaluates animation curves against on the CPU). The
        // GPU scripted-light pulse, SH animation, and animated-lightmap compose
        // all wrap this via `fract(time / period + phase)`. Using wall-clock
        // here instead would desync the GPU-rendered brightness from the CPU
        // `effective_brightness` that gates shadow-pool eligibility, so the pool
        // would shadow lights other than the ones actually lit on screen.
        // Full-ready path only (per-frame uniforms feed the scene render).
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("update_per_frame_uniforms requires full-ready renderer");

        #[cfg(not(feature = "dev-tools"))]
        let time = script_time;
        // Dev-tools: hold `time` when frozen (debug aid), else track live time so
        // toggling the freeze on holds the current animation phase.
        //
        // Freeze stops BOTH clocks together. While `freeze_time` is set, `App`
        // reads it (`renderer.freeze_time()`) and stops advancing `script_time`
        // (main.rs), so the CPU light bridge's `effective_brightness` (which
        // gates shadow-pool eligibility) and this GPU `time` uniform hold the
        // same phase. The held `frozen_time` here matches that pinned
        // `script_time`, so CPU and GPU stay aligned under freeze — no
        // animation-phase desync for a shadow debugger to chase.
        #[cfg(feature = "dev-tools")]
        let time = if full.freeze_time {
            full.frozen_time
        } else {
            full.frozen_time = script_time;
            full.frozen_time
        };
        // The per-light SDF visibility multiply is enabled whenever a baked SDF
        // atlas is loaded — the half-res target's four channels then hold valid
        // K = 4 per-light slices. With the flag clear (legacy PRL / no atlas)
        // the forward skips the upsample and treats every light fully lit.
        let mut sdf_shadow_flags: u32 = 0;
        if full.sdf_atlas_resources.present {
            sdf_shadow_flags |= SDF_SHADOW_FLAG_ATLAS_PRESENT;
        }
        // The diagnostics UI runs after this upload. Snapshot once so every
        // consumer of the mask observes a checkbox change together on the
        // following frame rather than mixing group-0's old value with live
        // post-UI state.
        full.frame_light_term_mask = full.light_term_mask;
        let data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position,
            ambient_floor: full.ambient_floor,
            light_count: full.light_count,
            total_light_count: full.total_light_count,
            time,
            light_term_mask: full.frame_light_term_mask,
            indirect_scale: full.indirect_scale,
            sdf_shadow_flags,
            sdf_shadow_mode: full.sdf_shadow_mode,
            sdf_force_visibility_one: full.sdf_force_visibility_one,
            dynamic_direct_scale: full.dynamic_direct_scale,
            has_direct: full.sh_volume_resources.direct.has_direct,
            spec_shadowmask_force_one: full.spec_shadowmask_force_one,
        });
        queue.write_buffer(&full.uniform_buffer, 0, &data);
        full.last_camera_position = camera_position;
        full.last_view_proj = view_proj;
        // Cache this frame's `time` so the skinned-mesh group-2 params uniform
        // (`MeshLightParams.time`) is written from the SAME render-clock value —
        // the scripted-light curves the mesh dynamic loop evaluates must share the
        // forward pass's animation phase (and the CPU light bridge's, which gates
        // shadow-pool eligibility). Written from this single source, never
        // recomputed at the mesh draw.
        full.mesh_dynamic_time = time;

        // Mesh dynamic-direct uniform (group 4 binding 16). The mesh path reads
        // a trimmed camera uniform (no group-0 tail), so its direct scale and
        // level-fixed `has_direct` flag reach it through this dedicated uniform.
        full.sh_volume_resources
            .direct
            .write_dynamic_direct_params(queue, full.dynamic_direct_scale);

        // Must precede the compose and SH fragment passes (both read the descriptor buffer).
        full.sh_volume_resources
            .animation
            .upload_descriptors_if_dirty(queue);
    }

    /// Upload the dedicated first-person weapon view-projection. This never
    /// reads game-side view-feel state: the caller has already folded that into
    /// the world-space `MeshInstanceInput` transform and supplies the matching
    /// render-camera view matrix.
    pub fn update_viewmodel_view_projection(&mut self, aspect: f32, view_matrix: Mat4) {
        let view_projection = viewmodel_projection(aspect) * view_matrix;
        let Self { queue, full, .. } = self;
        let full = full
            .as_ref()
            .expect("update_viewmodel_view_projection requires full-ready renderer");
        full.mesh_pass
            .write_viewmodel_view_projection(queue, view_projection);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewmodel_projection_maps_tight_near_and_far_planes_to_depth_range() {
        let projection = viewmodel_projection(16.0 / 9.0);
        for (distance, expected_depth) in [(VIEWMODEL_NEAR_CLIP, 0.0), (VIEWMODEL_FAR_CLIP, 1.0)] {
            let clip = projection * glam::Vec4::new(0.0, 0.0, -distance, 1.0);
            assert!(
                (clip.z / clip.w - expected_depth).abs() < 1e-5,
                "distance {distance} should map to depth {expected_depth}, got {}",
                clip.z / clip.w
            );
        }
    }

    #[test]
    fn viewmodel_world_transform_preserves_camera_space_clip_placement() {
        let projection = viewmodel_projection(16.0 / 9.0);
        let view = Mat4::look_at_rh(Vec3::new(4.0, 2.0, 7.0), Vec3::new(3.0, 2.5, 6.0), Vec3::Y);
        let camera_space_model = Mat4::from_translation(Vec3::new(0.3, -0.2, -0.6));
        let world_model = view.inverse() * camera_space_model;
        let model_point = Vec3::new(0.1, 0.05, -0.2).extend(1.0);

        let camera_space_clip = projection * camera_space_model * model_point;
        let world_space_clip = projection * view * world_model * model_point;

        assert!(
            camera_space_clip.distance(world_space_clip) < 1.0e-5,
            "world-space shading placement must preserve the dedicated viewmodel clip placement",
        );
    }
}

impl Renderer {
    #[cfg(feature = "dev-tools")]
    pub fn clear_debug_lines(&mut self) {
        self.full_mut().debug_lines.clear();
    }
}
