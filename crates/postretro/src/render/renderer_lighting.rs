// Lighting/SDF/shadow data logic for the renderer: dynamic-light filtering,
// shadow-candidate selection, slot assignment, and SH-grid metadata, plus the
// renderer light/fog/bridge upload methods.
// See: context/lib/rendering_pipeline.md §4

use super::*;

/// Pack the SH grid metadata the SDF shadow pass needs for its open-space
/// skip uniform. Mirrors what the forward pass reads from `ShGridInfo` (group
/// 3) — replicating it here lets the shadow pass keep group 3 off its
/// pipeline layout. Returns the "empty SH" defaults when the section is
/// absent or marked not-present, matching the dummy 1×1×1 path in
/// `ShVolumeResources`.
pub(crate) fn build_sdf_shadow_sh_grid(
    sh_volume: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    present: bool,
) -> SdfShadowShGrid {
    if !present {
        return SdfShadowShGrid::default();
    }
    let Some(sec) = sh_volume else {
        return SdfShadowShGrid::default();
    };
    SdfShadowShGrid {
        origin: sec.grid_origin,
        cell_size: sec.cell_size,
        dimensions: sec.grid_dimensions,
        has_volume: true,
    }
}

/// Per-light delta AABB overlays no longer have a source: the sparse CSR delta
/// format (v2) is keyed by affinity cell, not per-light AABB grids, so there are
/// no per-light origin/dims to draw. Returns empty; the diagnostics consumer
/// skips the delta-AABB loop. A future affinity-cell overlay could repopulate
/// this from `affinity_dims` + the base grid origin/cell-size.
#[cfg(feature = "dev-tools")]
pub(crate) fn collect_delta_volume_meta(
    _section: Option<&postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
) -> Vec<sh_volume::DeltaVolumeMeta> {
    Vec::new()
}

// Static lights are baked — including them would double-apply their contribution.
// Short influence list → zero-radius placeholder.
pub(crate) fn filter_dynamic_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> (Vec<MapLight>, Vec<LightInfluence>) {
    lights
        .iter()
        // enumerate before filter so i preserves the original index into influences
        .enumerate()
        .filter(|(_, l)| l.is_dynamic)
        .map(|(i, l)| {
            let inf = influences.get(i).cloned().unwrap_or(LightInfluence {
                center: Vec3::ZERO,
                radius: 0.0,
            });
            (l.clone(), inf)
        })
        .unzip()
}

/// Pull the spot-shadow pool's candidate set from the **full** level light
/// list: every dynamic-tier light (`is_dynamic`). A baked light's world shadow
/// is frozen in the lightmap, so it never needs a pool slot; only dynamic-tier
/// lights qualify.
///
/// Dynamic-tier spotlights cast world shadows through the shadow depth pass
/// (which renders static world geometry), so a pooled dynamic spot shadows
/// pillars and other occluders. The per-light `casts_entity_shadows` toggle
/// (FGD `_cast_entity_shadows`) is orthogonal to slot allocation — it gates
/// whether moving-ENTITY occluders are drawn into the already-allocated slot
/// (`entity_occluder_eligible`), not whether the slot exists.
///
/// Ranking is layered on top of the existing `eligible_lights`
/// visibility/brightness slice in `rank_lights`.
pub(crate) fn filter_entity_shadow_candidates(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> (Vec<MapLight>, Vec<LightInfluence>) {
    lights
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_dynamic)
        .map(|(i, l)| {
            let inf = influences.get(i).cloned().unwrap_or(LightInfluence {
                center: Vec3::ZERO,
                radius: 0.0,
            });
            (l.clone(), inf)
        })
        .unzip()
}

/// Identity-match a shadow candidate against the `level_lights` slice
/// (origin + light_type) and return that level-light's per-frame
/// effective brightness. Returns `None` when the candidate isn't in
/// `level_lights`. Both sets are `is_dynamic`-filtered snapshots of the same
/// `world.lights` source, so today every candidate is present and this returns
/// `Some`; the `None` arm is the defensive path for once light-movement
/// re-keying lands.
pub(crate) fn level_brightness_for_candidate(
    level_lights: &[MapLight],
    candidate: &MapLight,
    effective_brightness: &[f32],
) -> Option<f32> {
    // Re-keys by float-exact `origin` equality. Both `level_lights` and
    // `shadow_candidate_lights` are immutable load-time snapshots filtered from
    // the same `world.lights` source, so origins match exactly today. The match
    // breaks only once runtime light-movement lands and mutates one side's
    // origins live (the candidate snapshot would keep a stale origin and
    // silently lose the forward shadow slot). That feature doesn't exist —
    // `is_dynamic` is a dormant seam with no authoring surface and
    // `self.level_lights` is never mutated post-load — so keying on a stable id
    // now would be scaffolding for an unlanded feature. When movement lands, key
    // both sites on the `world.lights` source index (the natural shared id;
    // currently discarded by `filter_dynamic_lights` /
    // `filter_entity_shadow_candidates`) instead of origin equality.
    level_lights
        .iter()
        .enumerate()
        .find(|(_, l)| l.origin == candidate.origin && l.light_type == candidate.light_type)
        .and_then(|(i, _)| effective_brightness.get(i).copied())
}

/// Translate a slot assignment from candidate-index space into
/// `level_lights`-index space. Returns a Vec the size of `level_lights`,
/// each entry either a slot or `NO_SHADOW_SLOT`. Used to pack the GPU
/// lights buffer (`pack_lights_with_slots_into`), which is keyed on
/// `level_lights`. Candidates not in `level_lights` have no forward-side
/// slot today — that bridge is post-1b work.
pub(crate) fn slot_assignment_for_level_lights(
    level_lights: &[MapLight],
    candidates: &[MapLight],
    candidate_slot_assignment: &[u32],
) -> Vec<u32> {
    use crate::lighting::spot_shadow::NO_SHADOW_SLOT;
    let mut out = vec![NO_SHADOW_SLOT; level_lights.len()];
    for (cand_idx, &slot) in candidate_slot_assignment.iter().enumerate() {
        if slot == NO_SHADOW_SLOT {
            continue;
        }
        let cand = &candidates[cand_idx];
        // Re-keys by float-exact `origin` equality — same constraint as
        // `level_brightness_for_candidate`: exact today because both collections
        // are immutable load-time snapshots of the same `world.lights` source.
        // A moving spot (unlanded; see that fn) would carry a stale candidate
        // origin and silently drop its slot. Key both sites on the
        // `world.lights` source index when light-movement lands.
        if let Some((level_idx, _)) = level_lights
            .iter()
            .enumerate()
            .find(|(_, l)| l.origin == cand.origin && l.light_type == cand.light_type)
        {
            out[level_idx] = slot;
        }
    }
    out
}

impl Renderer {
    /// Flushed to GPU on the next `update_per_frame_uniforms` call.
    #[allow(dead_code)]
    pub fn set_animated_light_active(&mut self, slot: usize, active: bool) {
        self.full_mut()
            .sh_volume_resources
            .animation
            .set_active(slot, active);
    }

    /// Overwrite the entire 48-byte animation descriptor at `slot` in the
    /// animated-compose descriptor buffer. Used by the scripting bridge to
    /// route a `setLightAnimation` curve through the animated-baked compose
    /// path (Task 2c of `sdf-static-occluder-shadows`). Out-of-range slots
    /// log once then no-op (mirrors the dormant `set_active` behavior).
    /// Flushed to GPU on the next `update_per_frame_uniforms` call.
    pub fn write_animated_compose_descriptor(
        &mut self,
        slot: u32,
        bytes: &[u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE],
    ) {
        self.full_mut()
            .sh_volume_resources
            .animation
            .write_descriptor(slot as usize, bytes);
    }

    /// Must run before `update_dynamic_light_slots` — slot assignment reads
    /// then patches this buffer. If the order is reversed, `update_dynamic_light_slots`
    /// runs first and seeds `last_lights_upload` with static bytes; the subsequent
    /// bridge upload overwrites the mirror with animated base data but skips
    /// re-patching the shadow slot, so the bridge's sentinel slot persists and
    /// the forward shader never samples the shadow map for that frame.
    pub fn upload_bridge_lights(&mut self, lights_bytes: &[u8]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        debug_assert_eq!(
            lights_bytes.len(),
            full.level_lights.len() * GPU_LIGHT_SIZE,
            "bridge produced {} bytes; expected {} × {} = {}",
            lights_bytes.len(),
            full.level_lights.len(),
            GPU_LIGHT_SIZE,
            full.level_lights.len() * GPU_LIGHT_SIZE,
        );
        if lights_bytes.is_empty() {
            return;
        }
        queue.write_buffer(&full.lights_buffer, 0, lights_bytes);
        // Keep the CPU mirror in lock-step with the GPU buffer. The bridge
        // packs animated base data with sentinel shadow slots; the shadow pool
        // (`update_dynamic_light_slots`) then patches the real slot field onto
        // this mirror and re-uploads. Without this sync `last_lights_upload`
        // stays the wrong length or holds stale bytes: `update_dynamic_light_slots`
        // checks `last_lights_upload.len() == expected_len` and takes the fallback
        // full static-repack path when the lengths mismatch, clobbering the
        // animated base data written here with static bytes.
        full.last_lights_upload.clear();
        full.last_lights_upload.extend_from_slice(lights_bytes);
    }

    /// Mismatched length logs a warning and skips upload — fail soft over crashing the frame.
    pub fn upload_bridge_descriptors(&mut self, descriptor_bytes: &[u8]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run");
        let expected = full.level_lights.len() * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
        if descriptor_bytes.len() != expected {
            log::warn!(
                "[Renderer] upload_bridge_descriptors: bridge produced {} bytes; \
                 expected {} × {} = {}. Skipping upload.",
                descriptor_bytes.len(),
                full.level_lights.len(),
                sh_volume::ANIMATION_DESCRIPTOR_SIZE,
                expected,
            );
            return;
        }
        if descriptor_bytes.is_empty() {
            return;
        }
        queue.write_buffer(
            &full.sh_volume_resources.scripted_light_descriptors,
            0,
            descriptor_bytes,
        );
    }

    /// Writes at scripted-region offset (after FGD samples).
    pub fn upload_bridge_samples(&mut self, samples_bytes: &[u8]) {
        if samples_bytes.is_empty() {
            return;
        }
        let Self { queue, full, .. } = self;
        let full = full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run");
        let offset = full.sh_volume_resources.scripted_sample_byte_offset as u64;
        queue.write_buffer(
            &full.sh_volume_resources.animation.anim_samples,
            offset,
            samples_bytes,
        );
    }

    /// Divide by 4 for float index; pass as `fgd_sample_float_count` to `LightBridge`.
    pub fn scripted_sample_byte_offset(&self) -> usize {
        self.full().sh_volume_resources.scripted_sample_byte_offset
    }

    pub fn level_lights(&self) -> &[MapLight] {
        &self.full().level_lights
    }

    /// Collects dynamic spots with a shadow slot this frame.
    /// Unslotted spots excluded — no usable light-space matrix in the shader.
    /// Pre-multiplies color × intensity × brightness; mirrors `FogVolumeBridge::update_points`.
    pub(super) fn collect_fog_spot_lights(&self) -> Vec<crate::fx::fog_volume::FogSpotLight> {
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let full = self.full();
        let slot_assignment = &full.spot_shadow_pool.slot_assignment;
        if slot_assignment.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let Some(light) = full.level_lights.get(light_idx) else {
                continue;
            };
            if !matches!(light.light_type, crate::prl::LightType::Spot) {
                continue;
            }
            let multiplier = full
                .light_effective_brightness
                .get(light_idx)
                .copied()
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
            out.push(crate::fx::fog_volume::FogSpotLight {
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
        let stride = std::mem::size_of::<crate::fx::fog_volume::FogVolume>();
        if bytes.is_empty() {
            self.full_mut().fog.set_canonical_volumes(&[], &[], 0);
            return;
        }
        if bytes.len() % stride != 0 {
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
        let volumes: &[crate::fx::fog_volume::FogVolume] = bytemuck::cast_slice(bytes);
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
        let stride = std::mem::size_of::<crate::fx::fog_volume::FogPointLight>();
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        if bytes.is_empty() {
            full.fog.point_count = 0;
            return;
        }
        if bytes.len() % stride != 0 {
            log::warn!(
                "[Renderer] upload_fog_points: byte length {} is not a multiple of \
                 FogPointLight stride {}; skipping.",
                bytes.len(),
                stride,
            );
            full.fog.point_count = 0;
            return;
        }
        let points: &[crate::fx::fog_volume::FogPointLight] = bytemuck::cast_slice(bytes);
        full.fog.upload_points(queue, points);
    }

    /// Set the global `fog_pixel_scale` from worldspawn. No-op when unchanged.
    pub fn set_fog_pixel_scale(&mut self, scale: u32) {
        let Self {
            device,
            surface_config,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.fog.set_pixel_scale(
            device,
            scale,
            surface_config.width,
            surface_config.height,
            &full.depth_view,
        );
    }

    pub fn set_light_effective_brightness(&mut self, effective_brightness: &[f32]) {
        let full = self.full_mut();
        full.light_effective_brightness.clear();
        full.light_effective_brightness
            .extend_from_slice(effective_brightness);
    }
}
