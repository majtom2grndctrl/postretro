// Per-frame renderer plumbing: surface resize, per-frame uniform updates, and
// debug-line clearing.
// See: context/lib/rendering_pipeline.md §1

use super::*;

impl Renderer {
    /// Camera owns aspect ratio; caller must also call `update_per_frame_uniforms`.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        let (_depth_texture, depth_view) = create_depth_texture(&self.device, width, height);
        self.depth_view = depth_view;
        // `scene_color` is surface-sized; recreate it (and rebuild the resolve
        // bind group) alongside the depth target.
        self.screen_effects.resize(&self.device, width, height);
        self.fog
            .resize(&self.device, width, height, &self.depth_view);
        // SDF shadow target is half-res relative to the surface; the depth view
        // also changed, so the pass bind group has to be rebuilt.
        self.sdf_shadow_pass
            .resize(&self.device, &self.depth_view, width, height);
        // Group-5 bind group references both the SDF shadow factor target
        // and the scene depth — both just got recreated, so rebuild. The cube
        // binding's presence is fixed for the renderer's lifetime: the pool is
        // `Some` iff the adapter supports CUBE_ARRAY_TEXTURES, so rebuild the BGL
        // with the same flag (its presence mirrors the pool's).
        let cube_array_supported = self.cube_shadow_pool.is_some();
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(&self.device, cube_array_supported);
        // The cube sampling view is surface-size-independent, but the group-5
        // bind group is fully rebuilt here, so re-reference it (`Some` when the
        // pool is present, `None` omits binding 5 to match the BGL).
        let cube_sampling_view = self.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        self.spot_shadow_pool.rebuild_bind_group(
            &self.device,
            &spot_shadow_bgl,
            &self.sdf_shadow_pass.shadow_view,
            &self.depth_view,
            cube_sampling_view,
        );
        // The UI pass derives its device scale from `surface_config` at encode
        // time, so the splash needs no per-resize hook — it re-projects against
        // the new backbuffer size on the next `render_splash_frame`.
        self.is_surface_configured = true;
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
        let time = if self.freeze_time {
            self.frozen_time
        } else {
            self.frozen_time = script_time;
            self.frozen_time
        };
        // The per-light SDF visibility multiply is enabled whenever a baked SDF
        // atlas is loaded — the half-res target's four channels then hold valid
        // K = 4 per-light slices. With the flag clear (legacy PRL / no atlas)
        // the forward skips the upsample and treats every light fully lit.
        let mut sdf_shadow_flags: u32 = 0;
        if self.sdf_atlas_resources.present {
            sdf_shadow_flags |= SDF_SHADOW_FLAG_ATLAS_PRESENT;
        }
        let data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position,
            ambient_floor: self.ambient_floor,
            light_count: self.light_count,
            time,
            lighting_isolation: self.lighting_isolation,
            indirect_scale: self.indirect_scale,
            sdf_shadow_flags,
            sdf_shadow_mode: self.sdf_shadow_mode,
            sdf_force_visibility_one: self.sdf_force_visibility_one,
            dynamic_direct_scale: self.dynamic_direct_scale,
            dynamic_direct_isolation: self.dynamic_direct_isolation,
            has_direct: self.sh_volume_resources.has_direct,
        });
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
        self.last_camera_position = camera_position;
        self.last_view_proj = view_proj;
        // Cache this frame's `time` so the skinned-mesh group-2 params uniform
        // (`MeshLightParams.time`) is written from the SAME render-clock value —
        // the scripted-light curves the mesh dynamic loop evaluates must share the
        // forward pass's animation phase (and the CPU light bridge's, which gates
        // shadow-pool eligibility). Written from this single source, never
        // recomputed at the mesh draw.
        self.mesh_dynamic_time = time;

        // Mesh dynamic-direct uniform (group 4 binding 16). The mesh path reads
        // a trimmed camera uniform (no group-0 tail), so the scale/isolation/
        // has_direct knobs reach it through this dedicated uniform instead.
        self.sh_volume_resources.write_dynamic_direct_params(
            &self.queue,
            self.dynamic_direct_scale,
            self.dynamic_direct_isolation as u32,
        );

        // Must precede the compose and SH fragment passes (both read the descriptor buffer).
        self.sh_volume_resources
            .animation
            .upload_descriptors_if_dirty(&self.queue);
    }
}

impl Renderer {
    #[cfg(feature = "dev-tools")]
    pub fn clear_debug_lines(&mut self) {
        self.debug_lines.clear();
    }
}
