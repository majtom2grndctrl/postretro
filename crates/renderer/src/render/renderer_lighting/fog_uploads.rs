// Renderer fog uploads: fog volumes and points, cell masks, active AABBs, the
// shadowed fog spot set, and the worldspawn pixel scale.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;

impl Renderer {
    /// Collects dynamic spots with a shadow slot this frame.
    /// Unslotted spots excluded — no usable light-space matrix in the shader.
    /// Spots whose slot only the planar mirror holds are skipped too: fog is
    /// main-view only (`rendering_pipeline.md` §7.3a, Pool rank).
    /// Pre-multiplies color × intensity × brightness; mirrors `FogVolumeBridge::update_points`.
    pub(in crate::render) fn collect_fog_spot_lights(
        &self,
    ) -> Vec<postretro_render_cpu::fog_volume::FogSpotLight> {
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let full = self.full();
        let slot_assignment = &full.spot_shadow_pool.slot_assignment;
        if slot_assignment.is_empty() {
            return Vec::new();
        }
        // Fixed within the frame — build the source→level reverse lookup once so
        // each per-slot brightness read is O(1) instead of scanning
        // `level_light_source_indices`.
        let level_light_index_lookup =
            build_level_light_index_lookup(&full.level_light_source_indices);
        let mut out = Vec::new();
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            if shadow_candidate_is_promoted_baked(
                &full.shadow_candidate_selection_indices,
                &full.shadow_candidate_animated_baked_indices,
                light_idx,
            ) {
                continue;
            }
            // Fog is main-view only: a light the planar mirror alone holds a
            // slot for stays out, as an unslotted spot does.
            if full.mirror_lighting.is_mirror_only(light_idx) {
                continue;
            }
            let Some(light) = full.shadow_candidate_lights.get(light_idx) else {
                continue;
            };
            if !matches!(light.light_type, postretro_level_loader::LightType::Spot) {
                continue;
            }
            let multiplier = full
                .shadow_candidate_source_indices
                .get(light_idx)
                .and_then(|&source_index| {
                    level_brightness_for_candidate_indexed(
                        &level_light_index_lookup,
                        source_index,
                        &full.light_effective_brightness,
                    )
                })
                .unwrap_or(1.0);
            if multiplier < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                continue;
            }
            // Cull spots whose falloff sphere can't reach any active fog volume;
            // a non-overlapping spot contributes zero scatter in the raymarch.
            let center = Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            );
            if !sphere_intersects_any_fog_aabb(center, light.falloff_range, &full.active_fog_aabbs)
            {
                continue;
            }
            let intensity = light.intensity * multiplier;
            out.push(postretro_render_cpu::fog_volume::FogSpotLight {
                position: [
                    light.origin[0] as f32,
                    light.origin[1] as f32,
                    light.origin[2] as f32,
                ],
                slot,
                direction: light.cone_direction,
                cos_outer: light.cone_angle_outer.cos(),
                color: [
                    light.color[0] * intensity,
                    light.color[1] * intensity,
                    light.color[2] * intensity,
                ],
                range: light.falloff_range,
            });
        }
        out
    }

    /// Bytes: tightly packed `[FogVolume]` in PRL order. `live_mask` bit `i` = slot `i` has density > 0.
    /// GPU repack happens in `render_frame_indirect` after the portal-cull mask is known.
    /// Empty input clears the list → `FogPass::active` returns false.
    pub fn upload_fog_volumes(&mut self, bytes: &[u8], planes: &[Vec<[f32; 4]>], live_mask: u32) {
        let stride = std::mem::size_of::<postretro_render_cpu::fog_volume::FogVolume>();
        if bytes.is_empty() {
            self.full_mut().fog.set_canonical_volumes(&[], &[], 0);
            return;
        }
        if !bytes.len().is_multiple_of(stride) {
            log::warn!(
                "[Renderer] upload_fog_volumes: byte length {} is not a multiple of \
                 FogVolume stride {}; skipping.",
                bytes.len(),
                stride,
            );
            // Zero the canonical list — otherwise stale volumes from the previous frame persist.
            self.full_mut().fog.set_canonical_volumes(&[], &[], 0);
            return;
        }
        let volumes: &[postretro_render_cpu::fog_volume::FogVolume] = bytemuck::cast_slice(bytes);
        self.full_mut()
            .fog
            .set_canonical_volumes(volumes, planes, live_mask);
    }

    /// Installs per-cell fog visibility masks for a freshly loaded level and
    /// resets the fog pass's hysteresis timestamps in the same step.
    ///
    /// `None` = legacy PRL without section 31: all canonical slots treated active.
    /// `live_mask` still suppresses density-zero slots.
    ///
    /// Resetting hysteresis is part of the contract: without it, volumes from
    /// the previous level could ride the sticky window into the first frames
    /// of the new level. Because of that coupling, this method is only valid
    /// at level-load boundaries — mid-session fog-volume hot-reloads must use
    /// a different seam that preserves hysteresis state.
    pub fn install_fog_cell_masks_for_level(&mut self, masks: Option<Vec<u32>>) {
        let full = self.full_mut();
        full.fog_cell_masks = masks;
        full.fog.clear_for_level_load();
    }

    /// Must be called after bridge AABB cache is populated and before `collect_fog_spot_lights`.
    /// CPU-side culling data only — can't go through `upload_fog_volumes`.
    /// Empty slice clears the cache so spots aren't kept against a volume that turned off.
    pub fn set_fog_aabbs(&mut self, aabbs: &[(Vec3, Vec3)]) {
        let full = self.full_mut();
        full.active_fog_aabbs.clear();
        full.active_fog_aabbs.extend_from_slice(aabbs);
    }

    /// Bytes: tightly packed `[FogPointLight]`. Empty input zeroes `point_count`.
    pub fn upload_fog_points(&mut self, bytes: &[u8]) {
        let stride = std::mem::size_of::<postretro_render_cpu::fog_volume::FogPointLight>();
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        if bytes.is_empty() {
            full.fog.point_count = 0;
            return;
        }
        if !bytes.len().is_multiple_of(stride) {
            log::warn!(
                "[Renderer] upload_fog_points: byte length {} is not a multiple of \
                 FogPointLight stride {}; skipping.",
                bytes.len(),
                stride,
            );
            full.fog.point_count = 0;
            return;
        }
        let points: &[postretro_render_cpu::fog_volume::FogPointLight] =
            bytemuck::cast_slice(bytes);
        full.fog.upload_points(queue, points);
    }

    /// Set the global `fog_pixel_scale` from worldspawn. No-op when unchanged.
    pub fn set_fog_pixel_scale(&mut self, scale: u32) {
        // Fog scatter divides the scene extent, so a map's fog reads the same
        // at every render resolution.
        let scene = self.scene_extent();
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.fog
            .set_pixel_scale(device, scale, scene.width, scene.height, &full.depth_view);
    }
}
