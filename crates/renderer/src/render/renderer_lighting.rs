// Lighting/SDF/shadow data logic for the renderer: dynamic-light filtering,
// shadow-candidate selection, slot assignment, and SH-grid metadata, plus the
// renderer light/fog/bridge upload methods.
// See: context/lib/rendering_pipeline.md §4

use super::*;

const LIGHT_INFLUENCE_SIZE: usize = 16;

fn bridge_record_count(bytes_len: usize, stride: usize, capacity: usize) -> Option<usize> {
    if bytes_len % stride != 0 {
        return None;
    }
    let count = bytes_len / stride;
    (count <= capacity).then_some(count)
}

/// Validate the bridge upload that fills the compact dynamic descriptor prefix.
/// Promoted static records are appended to the light buffer but have no descriptor
/// slots, so an upload must end at `dynamic_light_count` rather than their total
/// forward-light count.
fn dynamic_descriptor_prefix_len(
    descriptor_bytes_len: usize,
    dynamic_light_count: u32,
    dynamic_light_capacity: usize,
) -> Option<usize> {
    let expected = dynamic_light_count as usize * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
    (descriptor_bytes_len == expected
        && bridge_record_count(
            descriptor_bytes_len,
            sh_volume::ANIMATION_DESCRIPTOR_SIZE,
            dynamic_light_capacity,
        )
        .is_some())
    .then_some(expected)
}

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

fn uncullable_light_influence() -> LightInfluence {
    LightInfluence {
        center: Vec3::ZERO,
        radius: f32::MAX,
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FilteredDynamicLights {
    pub lights: Vec<MapLight>,
    pub influences: Vec<LightInfluence>,
    /// Original index into the full level-light list for each filtered light.
    pub source_indices: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FilteredShadowCandidates {
    pub lights: Vec<MapLight>,
    pub influences: Vec<LightInfluence>,
    /// Original index into the full level-light list for each candidate.
    pub source_indices: Vec<usize>,
    /// Selection index for selected static lights; `None` for dynamic-tier
    /// candidates.
    pub selection_indices: Vec<Option<usize>>,
}

/// Shadow candidate reachability uses the runtime influence volume, not the
/// authored light origin/range. Missing influence entries are uncullable,
/// matching the loader/forward-light degradation contract.
pub(crate) fn shadow_candidate_reaches_visible_cell(
    light: &MapLight,
    influence: Option<&LightInfluence>,
    reachable_cell_aabbs: &[(Vec3, Vec3)],
) -> bool {
    if light.cell_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
        return false;
    }
    let influence = influence
        .cloned()
        .unwrap_or_else(uncullable_light_influence);
    postretro_lighting::light_reaches_visible_cell(
        influence.center,
        influence.radius,
        reachable_cell_aabbs,
    )
}

// Static lights are baked — including them would double-apply their contribution.
// Missing influence data means no spatial culling for that light.
pub(crate) fn filter_dynamic_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> FilteredDynamicLights {
    let mut filtered = FilteredDynamicLights::default();
    for (i, l) in lights.iter().enumerate().filter(|(_, l)| l.is_dynamic) {
        let inf = influences
            .get(i)
            .cloned()
            .unwrap_or_else(uncullable_light_influence);
        filtered.lights.push(l.clone());
        filtered.influences.push(inf);
        filtered.source_indices.push(i);
    }
    filtered
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
/// Ranking runs downstream in `assign_shadow_pool_slots_with_promoted_static`
/// (renderer_light_slots.rs): it scores this candidate slice and competes the
/// dynamic and promoted-static lights for the pool's slots.
#[cfg(test)]
pub(crate) fn filter_entity_shadow_candidates(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> FilteredShadowCandidates {
    filter_entity_shadow_candidates_with_selection(lights, influences, &[])
}

pub(crate) fn filter_entity_shadow_candidates_with_selection(
    lights: &[MapLight],
    influences: &[LightInfluence],
    entity_shadow_lights: &[u32],
) -> FilteredShadowCandidates {
    let mut filtered = FilteredShadowCandidates::default();
    for (i, l) in lights.iter().enumerate().filter(|(_, l)| l.is_dynamic) {
        let inf = influences
            .get(i)
            .cloned()
            .unwrap_or_else(uncullable_light_influence);
        filtered.lights.push(l.clone());
        filtered.influences.push(inf);
        filtered.source_indices.push(i);
        filtered.selection_indices.push(None);
    }
    // Compacted: a skipped selected-static entry is simply not a candidate. The
    // stamped `selection_index` is the RAW position in `entity_shadow_lights`,
    // so it stays aligned with the raw-length parallel vectors below.
    for (selection_index, &source_index) in entity_shadow_lights.iter().enumerate() {
        let source_index = source_index as usize;
        let Some((light, inf)) = resolve_selected_static_light(lights, influences, source_index)
        else {
            continue;
        };
        filtered.lights.push(light);
        filtered.influences.push(inf);
        filtered.source_indices.push(source_index);
        filtered.selection_indices.push(Some(selection_index));
    }
    filtered
}

/// Build the selected-static light/influence/source-index vectors index-parallel
/// to the selection index (0..N over `entity_shadow_lights`), NOT compacted: the
/// promotion driver, the per-selected-light weight buffer, and the baked delta
/// `affinity_lights` all key on the raw selection index, so every position must
/// occupy its slot. A skipped entry (dynamic-tier or a corrupt out-of-range
/// index) carries a sentinel that no candidate references — the driver never
/// promotes it, so the slot is never read; it exists only to keep the lookup
/// `[selection_index]` a direct aligned index.
pub(crate) fn filter_selected_static_entity_shadow_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
    entity_shadow_lights: &[u32],
) -> FilteredDynamicLights {
    let mut filtered = FilteredDynamicLights::default();
    for &source_index in entity_shadow_lights {
        let source_index = source_index as usize;
        match resolve_selected_static_light(lights, influences, source_index) {
            Some((light, inf)) => {
                filtered.lights.push(light);
                filtered.influences.push(inf);
                filtered.source_indices.push(source_index);
            }
            None => {
                filtered.lights.push(sentinel_selected_static_light());
                filtered.influences.push(uncullable_light_influence());
                filtered.source_indices.push(UNPROMOTABLE_SOURCE_INDEX);
            }
        }
    }
    filtered
}

/// Resolve one selected-static entry by its source index into the full level
/// light array. `Some(light, influence)` when the index is in range AND the
/// light is baked-tier (`!is_dynamic`); `None` when the entry can never promote
/// (out-of-range index, or a dynamic-tier light). Missing influence entries
/// fall back to uncullable, matching the loader/forward-light degradation
/// contract. Shared by the candidate filter (skips `None`) and the parallel
/// selected-static filter (fills `None` slots with a sentinel).
fn resolve_selected_static_light(
    lights: &[MapLight],
    influences: &[LightInfluence],
    source_index: usize,
) -> Option<(MapLight, LightInfluence)> {
    let light = lights.get(source_index)?;
    if light.is_dynamic {
        return None;
    }
    let influence = influences
        .get(source_index)
        .cloned()
        .unwrap_or_else(uncullable_light_influence);
    Some((light.clone(), influence))
}

/// Source-index marker for a selected-static slot that resolved to nothing.
/// Never referenced by a candidate, so never read as a promoted light index.
const UNPROMOTABLE_SOURCE_INDEX: usize = usize::MAX;

/// Placeholder light for a selected-static slot that resolved to nothing (a
/// dynamic-tier or out-of-range index in the `EntityShadowLights` section).
/// Keeps `filter_selected_static_entity_shadow_lights` index-parallel to the
/// selection index; the driver never promotes it, so it is never read.
fn sentinel_selected_static_light() -> MapLight {
    MapLight {
        origin: [0.0; 3],
        light_type: postretro_level_loader::LightType::Point,
        intensity: 0.0,
        color: [0.0; 3],
        falloff_model: postretro_level_loader::FalloffModel::Linear,
        falloff_range: 0.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0, 0.0, 1.0],
        is_dynamic: false,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: Vec::new(),
        cell_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    }
}

/// Build a reverse lookup from full-level-light source index to `level_lights`
/// position, so per-candidate brightness reads are O(1) instead of a linear
/// scan of `level_light_source_indices` (fixed within a frame). Sized to the
/// largest source index present; absent indices read back `None` (candidate not
/// represented in `level_lights`).
pub(crate) fn build_level_light_index_lookup(
    level_light_source_indices: &[usize],
) -> Vec<Option<usize>> {
    let len = level_light_source_indices
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m + 1);
    let mut lookup = vec![None; len];
    for (level_idx, &source_index) in level_light_source_indices.iter().enumerate() {
        if let Some(slot) = lookup.get_mut(source_index) {
            *slot = Some(level_idx);
        }
    }
    lookup
}

/// Look up a shadow candidate's per-frame effective brightness by its source
/// level-light index, via the reverse lookup from `build_level_light_index_lookup`.
/// Returns `None` when the candidate is not represented in `level_lights`. The
/// per-frame hot paths build the lookup once and call this directly, keeping the
/// per-candidate cost O(1).
pub(crate) fn level_brightness_for_candidate_indexed(
    level_light_index_lookup: &[Option<usize>],
    candidate_source_index: usize,
    effective_brightness: &[f32],
) -> Option<f32> {
    level_light_index_lookup
        .get(candidate_source_index)
        .copied()
        .flatten()
        .and_then(|level_idx| effective_brightness.get(level_idx).copied())
}

/// Translate a slot assignment from candidate-index space into
/// `level_lights`-index space. Returns a Vec the size of `level_lights`,
/// each entry either a slot or `NO_SHADOW_SLOT`. Used to pack the GPU
/// lights buffer (`pack_lights_with_slots_into`), which is keyed on the
/// filtered `level_lights` order.
pub(crate) fn slot_assignment_for_level_lights(
    level_light_source_indices: &[usize],
    candidate_source_indices: &[usize],
    candidate_slot_assignment: &[u32],
) -> Vec<u32> {
    use crate::lighting::spot_shadow::NO_SHADOW_SLOT;
    let mut out = vec![NO_SHADOW_SLOT; level_light_source_indices.len()];
    for (cand_idx, &slot) in candidate_slot_assignment.iter().enumerate() {
        if slot == NO_SHADOW_SLOT {
            continue;
        }
        let Some(&candidate_source_index) = candidate_source_indices.get(cand_idx) else {
            continue;
        };
        if let Some((level_idx, _)) = level_light_source_indices
            .iter()
            .enumerate()
            .find(|(_, source_index)| **source_index == candidate_source_index)
        {
            out[level_idx] = slot;
        }
    }
    out
}

pub(crate) fn shadow_candidate_is_promoted_static(
    selection_indices: &[Option<usize>],
    candidate_index: usize,
) -> bool {
    selection_indices
        .get(candidate_index)
        .and_then(|selection| *selection)
        .is_some()
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
    /// path. Out-of-range slots log once then no-op (mirrors the dormant
    /// `set_active` behavior).
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
        let Some(light_count) = bridge_record_count(
            lights_bytes.len(),
            GPU_LIGHT_SIZE,
            full.dynamic_light_capacity,
        ) else {
            log::warn!(
                "[Renderer] upload_bridge_lights: bridge produced {} bytes; expected a multiple \
                 of {} within the {}-record dynamic-light capacity. Skipping upload.",
                lights_bytes.len(),
                GPU_LIGHT_SIZE,
                full.dynamic_light_capacity,
            );
            return;
        };
        if !lights_bytes.is_empty() {
            queue.write_buffer(&full.lights_buffer, 0, lights_bytes);
        }
        full.light_count = light_count as u32;
        full.total_light_count = full.light_count + full.promoted_static_records.len() as u32;
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

    /// Upload the influence record paired with each compact dynamic light.
    /// The runtime-spawn reserve keeps this prefix in-bounds without rebinding.
    pub fn upload_bridge_influences(&mut self, influence_bytes: &[u8]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let expected = full.light_count as usize * LIGHT_INFLUENCE_SIZE;
        if influence_bytes.len() != expected
            || bridge_record_count(
                influence_bytes.len(),
                LIGHT_INFLUENCE_SIZE,
                full.dynamic_light_capacity,
            )
            .is_none()
        {
            log::warn!(
                "[Renderer] upload_bridge_influences: bridge produced {} bytes; expected {} \
                 dynamic records × {} = {}. Skipping upload.",
                influence_bytes.len(),
                full.light_count,
                LIGHT_INFLUENCE_SIZE,
                expected,
            );
            return;
        }
        if !influence_bytes.is_empty() {
            queue.write_buffer(&full.influence_buffer, 0, influence_bytes);
        }
        full.last_influence_upload.clear();
        full.last_influence_upload
            .extend_from_slice(influence_bytes);
    }

    /// Mismatched length logs a warning and skips upload — fail soft over crashing the frame.
    pub fn upload_bridge_descriptors(&mut self, descriptor_bytes: &[u8]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run");
        let Some(prefix_len) = dynamic_descriptor_prefix_len(
            descriptor_bytes.len(),
            full.light_count,
            full.dynamic_light_capacity,
        ) else {
            let expected = full.light_count as usize * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
            log::warn!(
                "[Renderer] upload_bridge_descriptors: bridge produced {} bytes; \
                 expected {} × {} = {}. Skipping upload.",
                descriptor_bytes.len(),
                full.light_count,
                sh_volume::ANIMATION_DESCRIPTOR_SIZE,
                expected,
            );
            return;
        };
        if prefix_len == 0 {
            return;
        }
        queue.write_buffer(
            &full.sh_volume_resources.scripted_light_descriptors,
            0,
            &descriptor_bytes[..prefix_len],
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
        let sample_capacity = full.sh_volume_resources.scripted_light_count as usize
            * postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT
            * std::mem::size_of::<f32>();
        if samples_bytes.len() > sample_capacity
            || samples_bytes.len()
                % (postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT
                    * std::mem::size_of::<f32>())
                != 0
        {
            log::warn!(
                "[Renderer] upload_bridge_samples: bridge produced {} bytes; scripted region \
                 capacity is {} bytes. Skipping upload.",
                samples_bytes.len(),
                sample_capacity,
            );
            return;
        }
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
    pub(super) fn collect_fog_spot_lights(
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
            if shadow_candidate_is_promoted_static(
                &full.shadow_candidate_selection_indices,
                light_idx,
            ) {
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
        let points: &[postretro_render_cpu::fog_volume::FogPointLight] =
            bytemuck::cast_slice(bytes);
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
        let expected = full.light_count as usize;
        if effective_brightness.len() != expected {
            log::warn!(
                "[Renderer] effective brightness count {} does not match dynamic light count {}; \
                 missing entries default to fully bright and excess entries are ignored.",
                effective_brightness.len(),
                expected,
            );
        }
        full.light_effective_brightness.clear();
        full.light_effective_brightness
            .extend(effective_brightness.iter().copied().take(expected));
        full.light_effective_brightness.resize(expected, 1.0);
    }
}

#[cfg(test)]
mod bridge_contract_tests {
    use super::*;

    #[test]
    fn bridge_record_count_accepts_zero_authored_runtime_records_within_reserve() {
        assert_eq!(
            bridge_record_count(
                GPU_LIGHT_SIZE,
                GPU_LIGHT_SIZE,
                RUNTIME_DYNAMIC_LIGHT_RESERVE
            ),
            Some(1),
        );
    }

    #[test]
    fn bridge_record_count_rejects_partial_and_over_capacity_uploads() {
        assert_eq!(
            bridge_record_count(GPU_LIGHT_SIZE - 1, GPU_LIGHT_SIZE, 4),
            None
        );
        assert_eq!(
            bridge_record_count(5 * GPU_LIGHT_SIZE, GPU_LIGHT_SIZE, 4),
            None
        );
    }

    // Regression: a dynamic-light despawn contracts the descriptor upload from
    // K+1 records to K, while promoted static records may occupy the stale K tail.
    #[test]
    fn dynamic_descriptor_prefix_contraction_excludes_stale_promoted_tail() {
        let stride = sh_volume::ANIMATION_DESCRIPTOR_SIZE;
        let mut gpu_bytes = vec![0u8; 3 * stride];
        gpu_bytes[stride..2 * stride].fill(0xAB); // Previous dynamic descriptor K.

        let dynamic_count_after_despawn = 1u32;
        let uploaded_prefix = dynamic_descriptor_prefix_len(stride, dynamic_count_after_despawn, 3)
            .expect("one live dynamic descriptor should fill exactly its prefix");
        gpu_bytes[..uploaded_prefix].fill(0xCD);

        assert_eq!(uploaded_prefix, stride);
        assert!(
            gpu_bytes[stride..2 * stride]
                .iter()
                .all(|&byte| byte == 0xAB),
            "the uploader intentionally leaves the old dynamic tail intact",
        );
        assert_eq!(
            dynamic_descriptor_prefix_len(2 * stride, dynamic_count_after_despawn, 3),
            None,
            "a promoted record after the contracted prefix must never gain a descriptor slot",
        );
    }
}
