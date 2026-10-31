// Scripting light-bridge uploads: snapshot validation, the bridge's light,
// influence, descriptor and sample writes, and the bridge-fed brightness gates.
// See: context/lib/rendering_pipeline.md §4

use super::*;

fn bridge_record_count(bytes_len: usize, stride: usize, capacity: usize) -> Option<usize> {
    if !bytes_len.is_multiple_of(stride) {
        return None;
    }
    let count = bytes_len / stride;
    (count <= capacity).then_some(count)
}

/// The bridge writes a dynamic prefix followed by one raw section-45 forward
/// record per `AnimatedBakedLights` row. Validate the two regions without
/// compacting roster holes into the dynamic namespace.
fn bridge_dynamic_prefix_count(
    bytes_len: usize,
    stride: usize,
    dynamic_capacity: usize,
    animated_baked_count: usize,
) -> Option<usize> {
    let total_count = bytes_len.checked_div(stride)?;
    if !bytes_len.is_multiple_of(stride) || total_count < animated_baked_count {
        return None;
    }
    let dynamic_count = total_count - animated_baked_count;
    (dynamic_count <= dynamic_capacity).then_some(dynamic_count)
}

/// Keep the bridge's authored animated-baked tail that follows the dynamic
/// prefix, so each frame's promotion weight scales the same base.
fn retain_authored_animated_tail(level: &mut LevelResourceSet, lights_bytes: &[u8]) {
    let dynamic_bytes = level.light_count as usize * GPU_LIGHT_SIZE;
    level.authored_animated_tail.clear();
    level
        .authored_animated_tail
        .extend_from_slice(lights_bytes.get(dynamic_bytes..).unwrap_or_default());
}

/// Validate the bridge upload that fills all forward descriptors: the compact
/// dynamic prefix plus the raw animated-baked tail. Promoted static records
/// append later and deliberately have no descriptors.
fn forward_descriptor_prefix_len(
    descriptor_bytes_len: usize,
    dynamic_light_count: u32,
    dynamic_light_capacity: usize,
    animated_baked_count: usize,
) -> Option<usize> {
    let expected = (dynamic_light_count as usize + animated_baked_count)
        * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
    (descriptor_bytes_len == expected
        && bridge_record_count(
            descriptor_bytes_len,
            sh_volume::ANIMATION_DESCRIPTOR_SIZE,
            dynamic_light_capacity + animated_baked_count,
        )
        .is_some())
    .then_some(expected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BridgeSnapshotLayout {
    dynamic_light_count: usize,
    descriptor_prefix_len: usize,
}

#[allow(clippy::too_many_arguments)]
fn validate_bridge_snapshot(
    lights_bytes_len: usize,
    influence_bytes_len: usize,
    descriptor_bytes: &[u8],
    samples_bytes_len: usize,
    effective_brightness_len: usize,
    animated_window_brightness_len: usize,
    compose_descriptor_writes: &[(u32, [u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE])],
    dynamic_light_capacity: usize,
    animated_baked_count: usize,
    scripted_light_capacity: usize,
    scripted_sample_float_offset: usize,
    compose_descriptor_count: u32,
) -> Option<BridgeSnapshotLayout> {
    let dynamic_light_count = bridge_dynamic_prefix_count(
        lights_bytes_len,
        GPU_LIGHT_SIZE,
        dynamic_light_capacity,
        animated_baked_count,
    )?;
    let influence_dynamic_count = bridge_dynamic_prefix_count(
        influence_bytes_len,
        LIGHT_INFLUENCE_SIZE,
        dynamic_light_capacity,
        animated_baked_count,
    )?;
    if influence_dynamic_count != dynamic_light_count {
        return None;
    }
    let descriptor_prefix_len = forward_descriptor_prefix_len(
        descriptor_bytes.len(),
        dynamic_light_count as u32,
        dynamic_light_capacity,
        animated_baked_count,
    )?;
    let forward_record_count = dynamic_light_count + animated_baked_count;
    if forward_record_count > scripted_light_capacity {
        return None;
    }
    let sample_slot_bytes =
        postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT * std::mem::size_of::<f32>();
    if !samples_bytes_len.is_multiple_of(sample_slot_bytes)
        || samples_bytes_len > scripted_light_capacity * sample_slot_bytes
        || !bridge_descriptors_fit_produced_samples(
            descriptor_bytes,
            compose_descriptor_writes,
            scripted_sample_float_offset,
            samples_bytes_len / std::mem::size_of::<f32>(),
        )
        || effective_brightness_len != dynamic_light_count
        || animated_window_brightness_len != animated_baked_count
        || compose_descriptor_writes
            .iter()
            .any(|(slot, _)| *slot >= compose_descriptor_count)
    {
        return None;
    }
    Some(BridgeSnapshotLayout {
        dynamic_light_count,
        descriptor_prefix_len,
    })
}

/// Bridge descriptors use absolute indices into the shared authored+scripted
/// sample buffer. A short but slot-aligned upload is safe only when every
/// descriptor supplied by the same transaction stays inside the newly
/// produced scripted extent; otherwise an accepted snapshot could read stale
/// samples left by a prior, longer bridge payload.
fn bridge_descriptors_fit_produced_samples(
    forward_descriptors: &[u8],
    compose_descriptor_writes: &[(u32, [u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE])],
    scripted_sample_float_offset: usize,
    produced_sample_float_count: usize,
) -> bool {
    let produced_end = match scripted_sample_float_offset.checked_add(produced_sample_float_count) {
        Some(end) => end,
        None => return false,
    };
    let descriptor_fits = |descriptor: &[u8]| {
        let read_u32 = |offset: usize| {
            u32::from_ne_bytes(
                descriptor[offset..offset + 4]
                    .try_into()
                    .expect("validated animation descriptor stride"),
            ) as usize
        };
        let range_fits = |offset: usize, count: usize, floats_per_sample: usize| {
            if count == 0 {
                return true;
            }
            offset >= scripted_sample_float_offset
                && count
                    .checked_mul(floats_per_sample)
                    .and_then(|len| offset.checked_add(len))
                    .is_some_and(|end| end <= produced_end)
        };

        range_fits(read_u32(8), read_u32(12), 1)
            && range_fits(read_u32(28), read_u32(32), 3)
            && range_fits(read_u32(40), read_u32(44), 3)
    };

    forward_descriptors
        .as_chunks::<{ sh_volume::ANIMATION_DESCRIPTOR_SIZE }>()
        .0
        .iter()
        .all(|descriptor| descriptor_fits(descriptor))
        && compose_descriptor_writes
            .iter()
            .all(|(_, descriptor)| descriptor_fits(descriptor))
}

impl Renderer {
    /// Number of real compose descriptors installed for the current level.
    /// Excludes the dummy buffer and becomes zero when SH installation degrades.
    pub fn animated_compose_descriptor_count(&self) -> u32 {
        self.full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run")
            .level
            .as_ref()
            .map_or(0, |level| {
                level.sh_volume_resources.animation.animated_light_count()
            })
    }

    /// Flushed to GPU on the next `update_per_frame_uniforms` call.
    #[allow(dead_code)]
    pub fn set_animated_light_active(&mut self, slot: usize, active: bool) {
        if let Some(level) = self.full_mut().level.as_mut() {
            level.sh_volume_resources.animation.set_active(slot, active);
        }
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
        if let Some(level) = self.full_mut().level.as_mut() {
            level
                .sh_volume_resources
                .animation
                .write_descriptor(slot as usize, bytes);
        }
    }

    /// Commit one coherent scripting-light snapshot. Every count and capacity
    /// is validated before the first GPU or CPU-mirror write; rejection leaves
    /// the previous lights, influences, descriptors, samples, compose records,
    /// and eligibility gates intact.
    #[allow(clippy::too_many_arguments)]
    #[must_use = "a rejected snapshot leaves the previous renderer state active"]
    pub fn upload_light_bridge_snapshot(
        &mut self,
        lights_bytes: &[u8],
        influence_bytes: &[u8],
        descriptor_bytes: &[u8],
        samples_bytes: &[u8],
        effective_brightness: &[f32],
        animated_window_brightness: &[f32],
        compose_descriptor_writes: &[(u32, [u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE])],
    ) -> bool {
        let validation = {
            let full = self.full();
            let Some(level) = full.level.as_ref() else {
                return false;
            };
            validate_bridge_snapshot(
                lights_bytes.len(),
                influence_bytes.len(),
                descriptor_bytes,
                samples_bytes.len(),
                effective_brightness.len(),
                animated_window_brightness.len(),
                compose_descriptor_writes,
                level.dynamic_light_capacity,
                level.animated_baked_light_count,
                level.sh_volume_resources.scripted_light_count as usize,
                level.sh_volume_resources.scripted_sample_byte_offset / std::mem::size_of::<f32>(),
                level.sh_volume_resources.animation.animated_light_count(),
            )
        };
        let Some(layout) = validation else {
            log::warn!(
                "[Renderer] rejected incomplete light-bridge snapshot; retaining the previous coherent snapshot"
            );
            return false;
        };

        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let Some(level) = full.level.as_mut() else {
            return false;
        };
        if !lights_bytes.is_empty() {
            queue.write_buffer(&level.lights_buffer, 0, lights_bytes);
        }
        if !influence_bytes.is_empty() {
            queue.write_buffer(&level.influence_buffer, 0, influence_bytes);
        }
        if layout.descriptor_prefix_len != 0 {
            queue.write_buffer(
                &level.sh_volume_resources.scripted_light_descriptors,
                0,
                &descriptor_bytes[..layout.descriptor_prefix_len],
            );
        }
        if !samples_bytes.is_empty() {
            queue.write_buffer(
                &level.sh_volume_resources.animation.anim_samples,
                level.sh_volume_resources.scripted_sample_byte_offset as u64,
                samples_bytes,
            );
        }

        level.light_count = layout.dynamic_light_count as u32;
        level.total_light_count = level.light_count
            + level.animated_baked_light_count as u32
            + level
                .promoted_baked_records
                .iter()
                .filter(|record| record.source.selected_static_index().is_some())
                .count() as u32;
        level.last_lights_upload.clear();
        level.last_lights_upload.extend_from_slice(lights_bytes);
        retain_authored_animated_tail(level, lights_bytes);
        level.last_influence_upload.clear();
        level
            .last_influence_upload
            .extend_from_slice(influence_bytes);
        for (slot, bytes) in compose_descriptor_writes {
            level
                .sh_volume_resources
                .animation
                .write_descriptor(*slot as usize, bytes);
        }
        level.light_effective_brightness.clear();
        level
            .light_effective_brightness
            .extend_from_slice(effective_brightness);
        level.animated_light_window_brightness.clear();
        level
            .animated_light_window_brightness
            .extend_from_slice(animated_window_brightness);
        true
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
        let Some(level) = full.level.as_mut() else {
            return;
        };
        let Some(light_count) = bridge_dynamic_prefix_count(
            lights_bytes.len(),
            GPU_LIGHT_SIZE,
            level.dynamic_light_capacity,
            level.animated_baked_light_count,
        ) else {
            log::warn!(
                "[Renderer] upload_bridge_lights: bridge produced {} bytes; expected a multiple \
                 of {} with its {}-record animated-baked tail inside the {}-record dynamic capacity. Skipping upload.",
                lights_bytes.len(),
                GPU_LIGHT_SIZE,
                level.animated_baked_light_count,
                level.dynamic_light_capacity,
            );
            return;
        };
        if !lights_bytes.is_empty() {
            queue.write_buffer(&level.lights_buffer, 0, lights_bytes);
        }
        level.light_count = light_count as u32;
        level.total_light_count = level.light_count
            + level.animated_baked_light_count as u32
            + level
                .promoted_baked_records
                .iter()
                .filter(|record| record.source.selected_static_index().is_some())
                .count() as u32;
        // Keep the CPU mirror in lock-step with the GPU buffer. The bridge
        // packs animated base data with sentinel shadow slots; the shadow pool
        // (`update_dynamic_light_slots`) then patches the real slot field onto
        // this mirror and re-uploads. Without this sync `last_lights_upload`
        // stays the wrong length or holds stale bytes: `update_dynamic_light_slots`
        // checks `last_lights_upload.len() == expected_len` and takes the fallback
        // full static-repack path when the lengths mismatch, clobbering the
        // animated base data written here with static bytes.
        level.last_lights_upload.clear();
        level.last_lights_upload.extend_from_slice(lights_bytes);
        retain_authored_animated_tail(level, lights_bytes);
    }

    /// Upload the influence record paired with each compact dynamic light.
    /// The runtime-spawn reserve keeps this prefix in-bounds without rebinding.
    pub fn upload_bridge_influences(&mut self, influence_bytes: &[u8]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let Some(level) = full.level.as_mut() else {
            return;
        };
        let expected =
            (level.light_count as usize + level.animated_baked_light_count) * LIGHT_INFLUENCE_SIZE;
        if influence_bytes.len() != expected
            || bridge_dynamic_prefix_count(
                influence_bytes.len(),
                LIGHT_INFLUENCE_SIZE,
                level.dynamic_light_capacity,
                level.animated_baked_light_count,
            )
            .is_none()
        {
            log::warn!(
                "[Renderer] upload_bridge_influences: bridge produced {} bytes; expected {} \
                 dynamic-plus-animated-forward records × {} = {}. Skipping upload.",
                influence_bytes.len(),
                level.light_count,
                LIGHT_INFLUENCE_SIZE,
                expected,
            );
            return;
        }
        if !influence_bytes.is_empty() {
            queue.write_buffer(&level.influence_buffer, 0, influence_bytes);
        }
        level.last_influence_upload.clear();
        level
            .last_influence_upload
            .extend_from_slice(influence_bytes);
    }

    /// Mismatched length logs a warning and skips upload — fail soft over crashing the frame.
    /// Returns whether the forward descriptor upload committed so callers can
    /// keep the paired compose descriptor writes transactional with it.
    #[must_use = "paired compose descriptor writes require a committed forward upload"]
    pub fn upload_bridge_descriptors(&mut self, descriptor_bytes: &[u8]) -> bool {
        let Self { queue, full, .. } = self;
        let full = full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run");
        let Some(level) = full.level.as_ref() else {
            return false;
        };
        let Some(prefix_len) = forward_descriptor_prefix_len(
            descriptor_bytes.len(),
            level.light_count,
            level.dynamic_light_capacity,
            level.animated_baked_light_count,
        ) else {
            let expected = (level.light_count as usize + level.animated_baked_light_count)
                * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
            log::warn!(
                "[Renderer] upload_bridge_descriptors: bridge produced {} bytes; \
                 expected {} × {} = {}. Skipping upload.",
                descriptor_bytes.len(),
                level.light_count,
                sh_volume::ANIMATION_DESCRIPTOR_SIZE,
                expected,
            );
            return false;
        };
        if prefix_len == 0 {
            return true;
        }
        queue.write_buffer(
            &level.sh_volume_resources.scripted_light_descriptors,
            0,
            &descriptor_bytes[..prefix_len],
        );
        true
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
        let Some(level) = full.level.as_ref() else {
            return;
        };
        let sample_capacity = level.sh_volume_resources.scripted_light_count as usize
            * postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT
            * std::mem::size_of::<f32>();
        if samples_bytes.len() > sample_capacity
            || !samples_bytes.len().is_multiple_of(
                postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT
                    * std::mem::size_of::<f32>(),
            )
        {
            log::warn!(
                "[Renderer] upload_bridge_samples: bridge produced {} bytes; scripted region \
                 capacity is {} bytes. Skipping upload.",
                samples_bytes.len(),
                sample_capacity,
            );
            return;
        }
        let offset = level.sh_volume_resources.scripted_sample_byte_offset as u64;
        queue.write_buffer(
            &level.sh_volume_resources.animation.anim_samples,
            offset,
            samples_bytes,
        );
    }

    /// Divide by 4 for float index; pass as `fgd_sample_float_count` to `LightBridge`.
    pub fn scripted_sample_byte_offset(&self) -> usize {
        self.full().level.as_ref().map_or(0, |level| {
            level.sh_volume_resources.scripted_sample_byte_offset
        })
    }

    pub fn level_lights(&self) -> &[MapLight] {
        self.full()
            .level
            .as_ref()
            .map_or(&[], |level| level.level_lights.as_slice())
    }

    pub fn set_light_effective_brightness(&mut self, effective_brightness: &[f32]) {
        let full = self.full_mut();
        let Some(level) = full.level.as_mut() else {
            return;
        };
        let expected = level.light_count as usize;
        if effective_brightness.len() != expected {
            log::warn!(
                "[Renderer] effective brightness count {} does not match dynamic light count {}; \
                 missing entries default to fully bright and excess entries are ignored.",
                effective_brightness.len(),
                expected,
            );
        }
        level.light_effective_brightness.clear();
        level
            .light_effective_brightness
            .extend(effective_brightness.iter().copied().take(expected));
        level.light_effective_brightness.resize(expected, 1.0);
    }

    /// Install the animated-baked lookahead brightness gate. This vector is
    /// keyed exclusively by `AnimatedBakedLights` index; it is intentionally a
    /// separate API from dynamic `effective_brightness`, whose compact index
    /// space excludes baked animated lights.
    pub fn set_animated_light_window_brightness(&mut self, brightness: &[f32]) {
        let full = self.full_mut();
        let Some(level) = full.level.as_mut() else {
            return;
        };
        let expected = level.promoted_animated_states.len();
        if brightness.len() != expected {
            log::warn!(
                "[Renderer] animated lookahead brightness count {} does not match section-45 animated-light count {}; \\
                 missing entries default to dark and excess entries are ignored.",
                brightness.len(),
                expected,
            );
        }
        level.animated_light_window_brightness.clear();
        level
            .animated_light_window_brightness
            .extend(brightness.iter().copied().take(expected));
        // Absent bridge data must fail closed: treating a baked animated light
        // as fully bright here would recreate the dynamic-index-space accident
        // this channel exists to avoid.
        level.animated_light_window_brightness.resize(expected, 0.0);
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
    // K+1 to K dynamic records, but its raw section-45 tail remains in place
    // and selected-static records may occupy the stale suffix after it.
    #[test]
    fn forward_descriptor_prefix_keeps_raw_animated_tail_and_excludes_static_suffix() {
        let stride = sh_volume::ANIMATION_DESCRIPTOR_SIZE;
        let animated_baked_count = 2;
        let mut gpu_bytes = vec![0u8; 4 * stride];
        gpu_bytes[3 * stride..4 * stride].fill(0xAB); // Stale selected-static suffix.

        let dynamic_count_after_despawn = 1u32;
        let uploaded_prefix = forward_descriptor_prefix_len(
            3 * stride,
            dynamic_count_after_despawn,
            3,
            animated_baked_count,
        )
        .expect(
            "one live dynamic plus two raw animated descriptors should fill exactly its prefix",
        );
        gpu_bytes[..uploaded_prefix].fill(0xCD);

        assert_eq!(uploaded_prefix, 3 * stride);
        assert!(
            gpu_bytes[3 * stride..4 * stride]
                .iter()
                .all(|&byte| byte == 0xAB),
            "the uploader intentionally leaves the selected-static suffix intact",
        );
        assert_eq!(
            forward_descriptor_prefix_len(
                2 * stride,
                dynamic_count_after_despawn,
                3,
                animated_baked_count,
            ),
            None,
            "every raw animated tail record must retain its descriptor slot",
        );
    }

    // Regression: a short forward descriptor upload retained the old GPU
    // descriptor while callers still committed its new compose-side partner.
    #[test]
    fn short_forward_descriptor_batch_is_rejected_before_compose_commit() {
        let stride = sh_volume::ANIMATION_DESCRIPTOR_SIZE;

        assert_eq!(
            forward_descriptor_prefix_len(stride, 1, 4, 1),
            None,
            "one dynamic plus one animated-tail descriptor is the atomic prefix",
        );
        assert_eq!(
            forward_descriptor_prefix_len(2 * stride, 1, 4, 1),
            Some(2 * stride),
        );
    }

    #[test]
    fn bridge_snapshot_rejects_partial_influence_and_gate_batches() {
        let descriptor = vec![0; sh_volume::ANIMATION_DESCRIPTOR_SIZE];
        assert_eq!(
            validate_bridge_snapshot(GPU_LIGHT_SIZE, 0, &descriptor, 0, 0, 1, &[], 2, 1, 2, 0, 0,),
            None,
            "the raw animated tail cannot commit without its influence",
        );
        assert_eq!(
            validate_bridge_snapshot(
                GPU_LIGHT_SIZE,
                LIGHT_INFLUENCE_SIZE,
                &descriptor,
                0,
                1,
                1,
                &[],
                2,
                1,
                2,
                0,
                0,
            ),
            None,
            "dynamic eligibility cannot partially accompany a static-only tail",
        );
    }

    #[test]
    fn bridge_dynamic_prefix_count_preserves_raw_animated_tail_at_every_roster_size() {
        assert_eq!(
            bridge_dynamic_prefix_count(5 * GPU_LIGHT_SIZE, GPU_LIGHT_SIZE, 4, 3),
            Some(2),
            "two dynamic records plus three raw animated entries stay index-parallel",
        );
        assert_eq!(
            bridge_dynamic_prefix_count(2 * GPU_LIGHT_SIZE, GPU_LIGHT_SIZE, 4, 3),
            None,
            "a partial raw tail must not be reinterpreted as compact dynamic records",
        );
    }

    #[test]
    fn bridge_snapshot_rejects_static_sample_capacity_shortfall_as_one_batch() {
        let descriptor = [0u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE];
        let compose = [(0, descriptor)];
        let sample_slot_bytes = postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT * 4;

        assert_eq!(
            validate_bridge_snapshot(
                GPU_LIGHT_SIZE,
                LIGHT_INFLUENCE_SIZE,
                &descriptor,
                3 * sample_slot_bytes,
                0,
                1,
                &compose,
                4,
                1,
                2,
                0,
                1,
            ),
            None,
            "a complete animated tail must not commit when static authored sample slots exceed capacity",
        );
    }

    // Regression: slot-aligned sample bytes were accepted even when a paired
    // descriptor reached into a stale slot left by an earlier, longer upload.
    #[test]
    fn bridge_snapshot_rejects_descriptor_reference_beyond_produced_samples() {
        let sample_slot_floats = postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT;
        let sample_slot_bytes = sample_slot_floats * std::mem::size_of::<f32>();
        let scripted_sample_float_offset = 7;
        let mut descriptor = [0u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE];
        let stale_slot_offset = scripted_sample_float_offset + sample_slot_floats;
        descriptor[8..12].copy_from_slice(&(stale_slot_offset as u32).to_ne_bytes());
        descriptor[12..16].copy_from_slice(&1u32.to_ne_bytes());

        assert_eq!(
            validate_bridge_snapshot(
                GPU_LIGHT_SIZE,
                LIGHT_INFLUENCE_SIZE,
                &descriptor,
                sample_slot_bytes,
                0,
                1,
                &[],
                4,
                1,
                2,
                scripted_sample_float_offset,
                0,
            ),
            None,
            "an aligned one-slot upload cannot expose a descriptor that references slot two",
        );

        descriptor[8..12].copy_from_slice(&(scripted_sample_float_offset as u32).to_ne_bytes());
        assert!(
            validate_bridge_snapshot(
                GPU_LIGHT_SIZE,
                LIGHT_INFLUENCE_SIZE,
                &descriptor,
                sample_slot_bytes,
                0,
                1,
                &[],
                4,
                1,
                2,
                scripted_sample_float_offset,
                0,
            )
            .is_some(),
            "the same transaction is valid once its descriptor stays inside the produced extent",
        );
    }
}
