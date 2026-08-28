//! Conservative, compiler-only eligibility policy for sparse SH delta entries.
//!
//! A missing CSR record is an existing wire-format spelling of a zero payload.
//! The first policy deliberately exploits only the exact-equivalence case: all
//! decoded RGB f16 texels in the 64-probe payload are zero.  That keeps the
//! omitted contribution at exactly zero through every runtime compose path,
//! including id-41's signed subtraction, the final non-negative clamp, and the
//! `rgba16float` store, without making an unproved claim about f16 rounding
//! thresholds for nonzero tiles.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{DeltaShVolumesSection, PROBES_PER_CELL};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::light_membership::LightMembershipManifest;
use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;

use crate::map_data::MapLight;

/// Descriptor slots whose animated entries are retained conservatively because
/// scripts can replace their curve after compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptMutableDescriptorSlots {
    pub indirect: Vec<bool>,
    pub animated_direct: Vec<bool>,
}

impl ScriptMutableDescriptorSlots {
    pub fn empty(slot_count: usize) -> Self {
        Self {
            indirect: vec![false; slot_count],
            animated_direct: vec![false; slot_count],
        }
    }

    fn mark(&mut self, slot: usize) {
        if let Some(value) = self.indirect.get_mut(slot) {
            *value = true;
        }
        if let Some(value) = self.animated_direct.get_mut(slot) {
            *value = true;
        }
    }

    pub(crate) fn indirect_contains(&self, slot: u32) -> bool {
        self.indirect.get(slot as usize).copied().unwrap_or(true)
    }

    pub(crate) fn animated_direct_contains(&self, slot: u32) -> bool {
        self.animated_direct
            .get(slot as usize)
            .copied()
            .unwrap_or(true)
    }
}

/// Derive mutable animated-baked slots from every script manifest target and
/// every authored `_animated` reservation.  `slot_for_map_light` must still be
/// in raw `MapData::lights` identity space: its values are the shared
/// descriptor/`AnimatedBakedLights` slots.  Runtime-only targets carry
/// `ANIMATED_SLOT_NONE` and deliberately mark nothing.
pub fn script_mutable_descriptor_slots(
    lights: &[MapLight],
    membership_manifest: Option<&LightMembershipManifest>,
    slot_for_map_light: &[u32],
    animated_slot_count: usize,
) -> ScriptMutableDescriptorSlots {
    let mut result = ScriptMutableDescriptorSlots::empty(animated_slot_count);
    let mut target = vec![false; lights.len()];
    for (index, light) in lights.iter().enumerate() {
        target[index] = light.is_animated;
    }
    if let Some(manifest) = membership_manifest {
        for record in &manifest.lights {
            if let Ok(index) = usize::try_from(record.index)
                && let Some(target) = target.get_mut(index)
            {
                // A manifest target may later receive setLightAnimation.  A
                // dynamic target maps to the runtime-only sentinel below.
                *target = true;
            }
        }
    }
    for (source_index, is_target) in target.into_iter().enumerate() {
        if !is_target {
            continue;
        }
        let Some(&slot) = slot_for_map_light.get(source_index) else {
            continue;
        };
        if slot != ANIMATED_SLOT_NONE {
            result.mark(slot as usize);
        }
    }
    result
}

/// Summary produced by a policy pass.  `largest_accepted_bound` is zero for
/// this exact-output policy, rather than a raw stored-payload threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropStats {
    pub input_entries: usize,
    pub retained_entries: usize,
    pub dropped_entries: usize,
    pub largest_accepted_bound: [f32; 3],
}

/// Rebuild id 27 in canonical cell order.  Mutable descriptors are retained
/// even when their current payload is zero: their future script curve may not
/// be zero.
pub fn drop_indirect_zero_entries(
    section: &DeltaShVolumesSection,
    mutable: &ScriptMutableDescriptorSlots,
) -> (DeltaShVolumesSection, DropStats) {
    let (offsets, lights, payload, stats) = rebuild_csr(
        section.affinity_offsets.as_slice(),
        section.affinity_lights.as_slice(),
        section.delta_subblocks.as_slice(),
        section.delta_probe_f16_stride(),
        |light, block| !mutable.indirect_contains(light) && rgb_payload_is_zero(block),
    );
    (
        DeltaShVolumesSection {
            affinity_factor: section.affinity_factor,
            affinity_dims: section.affinity_dims,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            animation_descriptor_indices: section.animation_descriptor_indices.clone(),
            valid_probe_masks: section.valid_probe_masks.clone(),
            cell_levels: section.cell_levels.clone(),
            affinity_offsets: offsets,
            affinity_lights: lights,
            delta_subblocks: payload,
        },
        stats,
    )
}

/// Rebuild id 45 in canonical cell order, preserving source index/payload
/// pairs.  The descriptor mask shares AnimatedBakedLights identity with id 45.
pub fn drop_animated_direct_zero_entries(
    section: &AnimatedDirectShDeltaVolumesSection,
    mutable: &ScriptMutableDescriptorSlots,
) -> (AnimatedDirectShDeltaVolumesSection, DropStats) {
    let (offsets, lights, payload, stats) = rebuild_csr(
        section.affinity_offsets.as_slice(),
        section.affinity_lights.as_slice(),
        section.delta_subblocks.as_slice(),
        section.delta_probe_f16_stride(),
        |light, block| !mutable.animated_direct_contains(light) && rgb_payload_is_zero(block),
    );
    (
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: section.affinity_factor,
            affinity_dims: section.affinity_dims,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            animation_descriptor_indices: section.animation_descriptor_indices.clone(),
            valid_probe_masks: section.valid_probe_masks.clone(),
            cell_levels: section.cell_levels.clone(),
            affinity_offsets: offsets,
            affinity_lights: lights,
            delta_subblocks: payload,
        },
        stats,
    )
}

/// Rebuild id 41, retaining each selection's highest canonical cell record if
/// every other record for that selection is exactly zero.  This preserves the
/// loader/promotion representation contract while still omitting redundant
/// zero records.
pub fn drop_direct_zero_entries(
    section: &DirectShDeltaVolumesSection,
) -> (DirectShDeltaVolumesSection, DropStats) {
    let stride = PROBES_PER_CELL * section.delta_probe_f16_stride();
    let mut final_entry_for_light = std::collections::BTreeMap::new();
    for (entry, &light) in section.affinity_lights.iter().enumerate() {
        final_entry_for_light.insert(light, entry);
    }
    let (offsets, lights, payload, stats) = rebuild_csr_indexed(
        section.affinity_offsets.as_slice(),
        section.affinity_lights.as_slice(),
        section.delta_subblocks.as_slice(),
        stride,
        |entry, light, block| {
            final_entry_for_light.get(&light).copied() != Some(entry) && rgb_payload_is_zero(block)
        },
    );
    (
        DirectShDeltaVolumesSection {
            affinity_factor: section.affinity_factor,
            affinity_dims: section.affinity_dims,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            valid_probe_masks: section.valid_probe_masks.clone(),
            cell_levels: section.cell_levels.clone(),
            affinity_offsets: offsets,
            affinity_lights: lights,
            delta_subblocks: payload,
        },
        stats,
    )
}

fn rebuild_csr(
    offsets: &[u32],
    lights: &[u32],
    payload: &[u16],
    probe_stride: usize,
    mut drop: impl FnMut(u32, &[u16]) -> bool,
) -> (Vec<u32>, Vec<u32>, Vec<u16>, DropStats) {
    rebuild_csr_indexed(
        offsets,
        lights,
        payload,
        PROBES_PER_CELL * probe_stride,
        |_, l, b| drop(l, b),
    )
}

fn rebuild_csr_indexed(
    offsets: &[u32],
    lights: &[u32],
    payload: &[u16],
    entry_stride: usize,
    mut drop: impl FnMut(usize, u32, &[u16]) -> bool,
) -> (Vec<u32>, Vec<u32>, Vec<u16>, DropStats) {
    assert_eq!(offsets.first(), Some(&0), "CSR must start at zero");
    assert_eq!(
        offsets.last().copied().unwrap_or_default() as usize,
        lights.len()
    );
    assert_eq!(payload.len(), lights.len() * entry_stride);
    let mut retained_lights = Vec::with_capacity(lights.len());
    let mut retained_payload = Vec::with_capacity(payload.len());
    let mut retained_offsets = Vec::with_capacity(offsets.len());
    retained_offsets.push(0);
    for cell in 0..offsets.len().saturating_sub(1) {
        let start = offsets[cell] as usize;
        let end = offsets[cell + 1] as usize;
        for entry in start..end {
            let block = &payload[entry * entry_stride..(entry + 1) * entry_stride];
            if !drop(entry, lights[entry], block) {
                retained_lights.push(lights[entry]);
                retained_payload.extend_from_slice(block);
            }
        }
        retained_offsets.push(retained_lights.len() as u32);
    }
    let stats = DropStats {
        input_entries: lights.len(),
        retained_entries: retained_lights.len(),
        dropped_entries: lights.len() - retained_lights.len(),
        largest_accepted_bound: [0.0; 3],
    };
    (retained_offsets, retained_lights, retained_payload, stats)
}

fn rgb_payload_is_zero(block: &[u16]) -> bool {
    // Alpha is structurally 1.0 in these tiles and has no radiance meaning.
    block
        .chunks_exact(4)
        .all(|rgba| rgba[..3].iter().all(|&half| f16_bits_to_f32(half) == 0.0))
}

fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 1;
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = bits & 0x3ff;
    let value = if exponent == 0 {
        mantissa as f32 * 2.0f32.powi(-24)
    } else if exponent == 0x1f {
        if mantissa == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + mantissa as f32 / 1024.0) * 2.0f32.powi(exponent as i32 - 15)
    };
    if sign == 0 { value } else { -value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{FalloffModel, LightAnimation, LightType, ShadowType};
    use glam::DVec3;
    use postretro_level_format::light_membership::{
        LightMembershipManifest, LightMembershipRecord,
    };
    use postretro_level_format::lightmap::f32_to_f16_bits;

    const TILE: u32 = 1;
    const STRIDE: usize = PROBES_PER_CELL * 4;

    fn block(rgb: [f32; 3]) -> Vec<u16> {
        let mut result = Vec::with_capacity(STRIDE);
        for _ in 0..PROBES_PER_CELL {
            result.extend(rgb.map(f32_to_f16_bits));
            result.push(f32_to_f16_bits(1.0));
        }
        result
    }

    fn indirect(lights: Vec<u32>, payload: Vec<u16>) -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [2, 1, 1],
            tile_dimension: TILE,
            tile_border: 0,
            animation_descriptor_indices: vec![0, 1, 2],
            valid_probe_masks: vec![u64::MAX; 2],
            cell_levels: vec![0u8; 2],
            affinity_offsets: vec![0, 2, lights.len() as u32],
            affinity_lights: lights,
            delta_subblocks: payload,
        }
    }

    fn direct(lights: Vec<u32>, payload: Vec<u16>) -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [2, 1, 1],
            tile_dimension: TILE,
            tile_border: 0,
            valid_probe_masks: vec![u64::MAX; 2],
            cell_levels: vec![0u8; 2],
            affinity_offsets: vec![0, 2, lights.len() as u32],
            affinity_lights: lights,
            delta_subblocks: payload,
        }
    }

    fn light(animated: bool, dynamic: bool) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0; 3],
            falloff_model: FalloffModel::Linear,
            falloff_range: 1.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: animated.then(|| LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: None,
                color: None,
                direction: None,
                start_active: true,
            }),
            bake_only: false,
            is_dynamic: dynamic,
            casts_entity_shadows: false,
            is_animated: animated,
            tags: vec![],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn direct_f16_output_error(base: f32, delta: f32, weight: f32) -> f32 {
        let weight = weight.clamp(0.0, 1.0);
        let before = f16_bits_to_f32(f32_to_f16_bits(base.max(0.0)));
        let after = f16_bits_to_f32(f32_to_f16_bits((base - delta * weight).max(0.0)));
        (before - after).abs()
    }

    #[test]
    fn exact_zero_drop_is_safe_for_signed_clamped_f16_direct_compose() {
        for weight in [0.0, 0.37, 1.0] {
            assert_eq!(direct_f16_output_error(0.0002, 0.0, weight), 0.0);
            assert_eq!(direct_f16_output_error(1.0, 0.0, weight), 0.0);
        }
        // The interior case crosses the clamp boundary for a nonzero delta;
        // keeping it demonstrates why only exact zero is accepted today.
        assert!(direct_f16_output_error(0.003, 0.01, 0.37) > 0.001);
    }

    #[test]
    fn cumulative_zero_records_are_still_zero_at_every_texel() {
        let mut payload = block([0.0; 3]);
        payload.extend(block([0.0; 3]));
        let section = indirect(vec![0, 1], payload);
        let (dropped, stats) =
            drop_indirect_zero_entries(&section, &ScriptMutableDescriptorSlots::empty(3));
        assert_eq!(stats.dropped_entries, 2);
        assert_eq!(dropped.affinity_offsets, vec![0, 0, 0]);
        assert!(dropped.delta_subblocks.is_empty());
    }

    #[test]
    fn mutable_manifest_and_animated_reservations_are_retained() {
        let lights = vec![light(false, false), light(false, true), light(true, false)];
        let manifest = LightMembershipManifest::new(
            vec![
                LightMembershipRecord {
                    index: 0,
                    is_dynamic: false,
                    start_active: None,
                    start_active_conflict: false,
                },
                LightMembershipRecord {
                    index: 1,
                    is_dynamic: true,
                    start_active: None,
                    start_active_conflict: false,
                },
            ],
            vec![],
        );
        let mask = script_mutable_descriptor_slots(
            &lights,
            Some(&manifest),
            &[0, ANIMATED_SLOT_NONE, 1],
            2,
        );
        assert_eq!(mask.indirect, vec![true, true]);
        assert_eq!(mask.animated_direct, vec![true, true]);
    }

    #[test]
    fn canonical_rebuild_preserves_index_payload_pairs_and_empty_cells() {
        let mut payload = block([0.0; 3]);
        payload.extend(block([0.25, 0.0, 0.0]));
        payload.extend(block([0.0; 3]));
        let section = indirect(vec![0, 2, 1], payload.clone());
        let (rebuilt, stats) =
            drop_indirect_zero_entries(&section, &ScriptMutableDescriptorSlots::empty(3));
        assert_eq!(stats.dropped_entries, 2);
        assert_eq!(rebuilt.affinity_offsets, vec![0, 1, 1]);
        assert_eq!(rebuilt.affinity_lights, vec![2]);
        assert_eq!(rebuilt.delta_subblocks, payload[STRIDE..STRIDE * 2]);
    }

    #[test]
    fn direct_keeps_highest_canonical_entry_for_each_selection() {
        let mut payload = block([0.0; 3]);
        payload.extend(block([0.0; 3]));
        payload.extend(block([0.0; 3]));
        let section = direct(vec![7, 7, 2], payload);
        let (rebuilt, stats) = drop_direct_zero_entries(&section);
        assert_eq!(stats.dropped_entries, 1);
        assert_eq!(rebuilt.affinity_offsets, vec![0, 1, 2]);
        assert_eq!(rebuilt.affinity_lights, vec![7, 2]);
    }

    #[test]
    fn animated_direct_obeys_the_same_mutable_retention_rule() {
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: TILE,
            tile_border: 0,
            animation_descriptor_indices: vec![0, 1],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: [block([0.0; 3]), block([0.0; 3])].concat(),
        };
        let mut mutable = ScriptMutableDescriptorSlots::empty(2);
        mutable.mark(1);
        let (rebuilt, stats) = drop_animated_direct_zero_entries(&section, &mutable);
        assert_eq!(stats.dropped_entries, 1);
        assert_eq!(rebuilt.affinity_offsets, vec![0, 1]);
        assert_eq!(rebuilt.affinity_lights, vec![1]);
    }
}
