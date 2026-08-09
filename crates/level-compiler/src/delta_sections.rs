//! Carries all baked delta sections through the compiler's post-bake seam.
//! See: context/lib/build_pipeline.md §PRL section IDs.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;

use crate::delta_drop_policy::{
    DropStats, ScriptMutableDescriptorSlots, drop_animated_direct_zero_entries,
    drop_direct_zero_entries, drop_indirect_zero_entries,
};

/// Default aggregate raw payload cap for ids 27, 41, and 45 on desktop maps.
pub(crate) const DEFAULT_MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// Compiler-only configuration for the post-bake delta-section policy.
///
/// The cap is carried with the baked sections so the policy can enforce it
/// before packing without leaking an implementation choice into the PRL wire
/// format or runtime representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeltaSectionConfig {
    pub max_payload_bytes: u64,
}

impl Default for DeltaSectionConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
        }
    }
}

/// Delta sections after all three delta bakes have completed.
///
/// This is intentionally an owned handoff. The next compiler-only policy can
/// inspect or replace these sections before packing while the pipeline and
/// runtime continue to consume the established dense section contracts.
pub(crate) struct PostBakeDeltaSections {
    pub config: DeltaSectionConfig,
    pub indirect: Option<DeltaShVolumesSection>,
    pub entity_shadow_lights: Option<EntityShadowLightsSection>,
    pub direct: Option<DirectShDeltaVolumesSection>,
    pub animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
}

impl PostBakeDeltaSections {
    pub(crate) fn new(
        config: DeltaSectionConfig,
        indirect: Option<DeltaShVolumesSection>,
        entity_shadow_lights: Option<EntityShadowLightsSection>,
        direct: Option<DirectShDeltaVolumesSection>,
        animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
    ) -> Self {
        Self {
            config,
            indirect,
            entity_shadow_lights,
            direct,
            animated_direct,
        }
    }

    /// Apply the compiler-only exact-zero policy while all delta sections are
    /// still owned by the pipeline.  The wire spelling of a dropped record is
    /// the existing CSR absence spelling for a zero contribution; retained
    /// records preserve their fixed dense payload stride.
    pub(crate) fn apply_exact_zero_drop_policy(
        &mut self,
        mutable_descriptors: &ScriptMutableDescriptorSlots,
    ) -> anyhow::Result<()> {
        if let Some(section) = self.indirect.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let (section, stats) = drop_indirect_zero_entries(&section, mutable_descriptors);
            log_drop_summary(
                "DeltaShVolumes",
                stats,
                input_payload_bytes,
                payload_bytes(&section.delta_subblocks),
                csr_bytes(&section.affinity_offsets, &section.affinity_lights),
                false,
            );
            // id 27's existing base-only spelling is an emitted empty section.
            self.indirect = Some(section);
        }

        if let Some(section) = self.direct.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let (section, stats) = drop_direct_zero_entries(&section);
            log_drop_summary(
                "DirectShDeltaVolumes",
                stats,
                input_payload_bytes,
                payload_bytes(&section.delta_subblocks),
                csr_bytes(&section.affinity_offsets, &section.affinity_lights),
                false,
            );
            self.direct = Some(section);
        }

        if let Some(section) = self.animated_direct.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let (section, stats) = drop_animated_direct_zero_entries(&section, mutable_descriptors);
            let is_empty = section.affinity_lights.is_empty();
            log_drop_summary(
                "AnimatedDirectShDeltaVolumes",
                stats,
                input_payload_bytes,
                payload_bytes(&section.delta_subblocks),
                csr_bytes(&section.affinity_offsets, &section.affinity_lights),
                is_empty,
            );
            // id 45 has an optional wire contract: an empty post-drop section
            // is absent, rather than a header with no CSR records.
            self.animated_direct = (!is_empty).then_some(section);
        }

        self.enforce_payload_cap()
    }

    /// Enforce the explicit desktop budget on raw dense delta blocks only.
    /// Header, descriptor, CSR, and unrelated PRL section bytes deliberately
    /// do not participate in this cap.
    pub(crate) fn enforce_payload_cap(&self) -> anyhow::Result<()> {
        let indirect = self
            .indirect
            .as_ref()
            .map_or(0, |section| payload_bytes(&section.delta_subblocks));
        let direct = self
            .direct
            .as_ref()
            .map_or(0, |section| payload_bytes(&section.delta_subblocks));
        let animated_direct = self
            .animated_direct
            .as_ref()
            .map_or(0, |section| payload_bytes(&section.delta_subblocks));
        let total = indirect
            .checked_add(direct)
            .and_then(|bytes| bytes.checked_add(animated_direct))
            .ok_or_else(|| anyhow::anyhow!("SH delta payload byte total overflow"))?;
        let overage = total.saturating_sub(self.config.max_payload_bytes);
        log::info!(
            "[Compiler] SH delta payload cap: id 27 {indirect} bytes, id 41 {direct} bytes, \\
             id 45 {animated_direct} bytes, total {total} bytes, cap {} bytes, overage {overage} bytes",
            self.config.max_payload_bytes,
        );
        anyhow::ensure!(
            total <= self.config.max_payload_bytes,
            "SH delta payload cap exceeded before packing: id 27 {indirect} bytes, id 41 {direct} bytes, \\
             id 45 {animated_direct} bytes; total {total} bytes exceeds cap {} bytes by {overage} bytes",
            self.config.max_payload_bytes,
        );
        Ok(())
    }
}

fn payload_bytes(payload: &[u16]) -> u64 {
    u64::try_from(payload.len()).expect("payload length fits u64") * size_of::<u16>() as u64
}

fn csr_bytes(offsets: &[u32], lights: &[u32]) -> u64 {
    u64::try_from(offsets.len() + lights.len()).expect("CSR length fits u64")
        * size_of::<u32>() as u64
}

fn log_drop_summary(
    name: &str,
    stats: DropStats,
    input_payload_bytes: u64,
    retained_payload_bytes: u64,
    retained_csr_bytes: u64,
    normalized_absent: bool,
) {
    let emitted = if normalized_absent {
        "absent"
    } else {
        "present"
    };
    log::info!(
        "[Compiler] {name} delta drop: input {} entry/entries ({input_payload_bytes} raw payload bytes); \\
         retained {} and dropped {}; retained {retained_payload_bytes} raw payload bytes, \\
         {retained_csr_bytes} CSR bytes, largest accepted RGB bound [{:.6}, {:.6}, {:.6}], {emitted}",
        stats.input_entries,
        stats.retained_entries,
        stats.dropped_entries,
        stats.largest_accepted_bound[0],
        stats.largest_accepted_bound[1],
        stats.largest_accepted_bound[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
    use postretro_level_format::delta_sh_volumes::{
        DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };
    use postretro_level_format::lightmap::f32_to_f16_bits;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };

    use crate::delta_drop_policy::ScriptMutableDescriptorSlots;

    const TILE: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;
    const ENTRY_STRIDE: usize = PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE;

    fn block(rgb: [f32; 3]) -> Vec<u16> {
        let mut payload = Vec::with_capacity(ENTRY_STRIDE);
        for _ in 0..ENTRY_STRIDE / 4 {
            payload.extend(rgb.map(f32_to_f16_bits));
            payload.push(f32_to_f16_bits(1.0));
        }
        payload
    }

    fn indirect(entries: Vec<u32>, payload: Vec<u16>) -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [entries.len() as u32, 1, 1],
            tile_dimension: TILE,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX; entries.len()],
            affinity_offsets: (0..=entries.len()).map(|index| index as u32).collect(),
            affinity_lights: entries,
            delta_subblocks: payload,
        }
    }

    fn direct(entries: Vec<u32>, payload: Vec<u16>) -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [entries.len() as u32, 1, 1],
            tile_dimension: TILE,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![u64::MAX; entries.len()],
            affinity_offsets: (0..=entries.len()).map(|index| index as u32).collect(),
            affinity_lights: entries,
            delta_subblocks: payload,
        }
    }

    fn animated_direct(
        entries: Vec<u32>,
        payload: Vec<u16>,
    ) -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [entries.len() as u32, 1, 1],
            tile_dimension: TILE,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX; entries.len()],
            affinity_offsets: (0..=entries.len()).map(|index| index as u32).collect(),
            affinity_lights: entries,
            delta_subblocks: payload,
        }
    }

    fn empty_indirect() -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![],
            valid_probe_masks: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    fn empty_direct() -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    fn empty_animated_direct() -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![],
            valid_probe_masks: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    #[test]
    fn post_bake_handoff_preserves_sections_and_owns_the_cap() {
        let config = DeltaSectionConfig {
            max_payload_bytes: 123,
        };
        let indirect = empty_indirect();
        let entity_shadow_lights = EntityShadowLightsSection {
            light_indices: vec![2, 7],
        };
        let direct = empty_direct();
        let animated_direct = empty_animated_direct();

        let handoff = PostBakeDeltaSections::new(
            config,
            Some(indirect.clone()),
            Some(entity_shadow_lights.clone()),
            Some(direct.clone()),
            Some(animated_direct.clone()),
        );

        assert_eq!(handoff.config, config);
        assert_eq!(handoff.indirect, Some(indirect));
        assert_eq!(handoff.entity_shadow_lights, Some(entity_shadow_lights));
        assert_eq!(handoff.direct, Some(direct));
        assert_eq!(handoff.animated_direct, Some(animated_direct));
    }

    #[test]
    fn delta_section_config_defaults_to_desktop_cap() {
        assert_eq!(
            DeltaSectionConfig::default().max_payload_bytes,
            256 * 1024 * 1024
        );
    }

    #[test]
    fn policy_keeps_id27_empty_but_normalizes_empty_id45_to_absent() {
        let zero = block([0.0; 3]);
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            Some(indirect(vec![0], zero.clone())),
            None,
            None,
            Some(animated_direct(vec![0], zero)),
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(1))
            .expect("zero payloads fit the default cap");

        let indirect = sections
            .indirect
            .expect("id 27 preserves its empty spelling");
        assert!(indirect.affinity_lights.is_empty());
        assert_eq!(indirect.affinity_offsets, vec![0, 0]);
        assert!(sections.animated_direct.is_none());
    }

    #[test]
    fn policy_preserves_id41_selection_coverage_and_loader_shape() {
        let zero = block([0.0; 3]);
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            None,
            Some(EntityShadowLightsSection {
                light_indices: vec![42],
            }),
            Some(direct(vec![0, 0], [zero.clone(), zero].concat())),
            None,
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(0))
            .expect("covered zero direct section fits the default cap");

        let direct = sections.direct.expect("id 41 remains present");
        assert_eq!(direct.affinity_lights, vec![0]);
        assert!(crate::pack::direct_sh_delta_covers_selection(&direct, 1));
        assert!(crate::pack::direct_sh_delta_has_valid_csr_shape(&direct));
        let decoded = DirectShDeltaVolumesSection::from_bytes(&direct.to_bytes())
            .expect("retained id 41 uses the existing loader format");
        assert_eq!(decoded, direct);
    }

    #[test]
    fn policy_output_is_deterministic_and_retains_nonzero_payload_pairs() {
        let zero = block([0.0; 3]);
        let nonzero = block([0.25, 0.0, 0.0]);
        let make_sections = || {
            PostBakeDeltaSections::new(
                DeltaSectionConfig::default(),
                Some(indirect(
                    vec![0, 0, 0],
                    [zero.clone(), nonzero.clone(), zero.clone()].concat(),
                )),
                None,
                None,
                None,
            )
        };
        let mut first = make_sections();
        let mut second = make_sections();
        let mutable = ScriptMutableDescriptorSlots::empty(1);
        first
            .apply_exact_zero_drop_policy(&mutable)
            .expect("small payload fits cap");
        second
            .apply_exact_zero_drop_policy(&mutable)
            .expect("small payload fits cap");

        let first = first.indirect.expect("id 27 is emitted");
        let second = second.indirect.expect("id 27 is emitted");
        assert_eq!(first.affinity_offsets, vec![0, 0, 1, 1]);
        assert_eq!(first.affinity_lights, vec![0]);
        assert_eq!(first.delta_subblocks, nonzero);
        assert_eq!(first.to_bytes(), second.to_bytes());
    }

    #[test]
    fn cap_rejection_reports_every_delta_section_before_packing() {
        let nonzero = block([0.25, 0.0, 0.0]);
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig {
                max_payload_bytes: payload_bytes(&nonzero) - 1,
            },
            Some(indirect(vec![0], nonzero)),
            None,
            None,
            None,
        );

        let error = sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(1))
            .expect_err("the retained raw payload is over the explicit cap");
        let message = error.to_string();
        assert!(message.contains("id 27"));
        assert!(message.contains("id 41"));
        assert!(message.contains("id 45"));
        assert!(message.contains("exceeds cap"));
    }
}
