//! Conservative, compiler-only eligibility policy for sparse SH delta entries.
//!
//! A missing CSR record is an existing wire-format spelling of a zero payload.
//! The first policy deliberately exploits only the exact-equivalence case: all
//! decoded RGB f16 texels in the 64-probe payload are zero.  That keeps the
//! omitted contribution at exactly zero through every runtime compose path,
//! including id-41's signed subtraction, the final non-negative clamp, and the
//! `rgba16float` store.  It is therefore strictly inside the fixed 0.001 RGB
//! error budget without making an unproved claim about f16 rounding thresholds.
//!
//! The descriptor extrema and script-mutability helpers below are intentionally
//! kept with the policy.  A later non-zero eligibility rule must consume these
//! bounds rather than treating raw unit-radiance tiles as their runtime scale.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{DeltaShVolumesSection, PROBES_PER_CELL};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::light_membership::LightMembershipManifest;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::sh_volume::{ANIMATED_SLOT_NONE, AnimationDescriptor};

use crate::map_data::MapLight;

/// Fixed, componentwise runtime-output budget.  Do not tune this to make a map
/// fit a size target.
pub const MAX_OUTPUT_ERROR: f32 = 0.001;

/// Descriptor slots whose animated entries must never be omitted: scripts can
/// replace their curve after compilation, so authored extrema are not a bound.
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

    fn indirect_contains(&self, slot: u32) -> bool {
        self.indirect.get(slot as usize).copied().unwrap_or(true)
    }

    fn animated_direct_contains(&self, slot: u32) -> bool {
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

/// Per-channel absolute scale for unit-radiance animated delta tiles over the
/// complete curve cycle.  The runtime clamps brightness and color samples to
/// non-negative before multiplication, so this is an upper bound on the
/// magnitude of an immutable descriptor's contribution.
pub fn immutable_descriptor_scale_bound(desc: &AnimationDescriptor) -> [f32; 3] {
    let open = desc.period < 0.0;
    let brightness = curve_nonnegative_supremum(&desc.brightness, 1.0, open);
    let color = if desc.color.is_empty() {
        [1.0; 3]
    } else {
        [
            curve_nonnegative_supremum_rgb(&desc.color, 0, open),
            curve_nonnegative_supremum_rgb(&desc.color, 1, open),
            curve_nonnegative_supremum_rgb(&desc.color, 2, open),
        ]
    };
    [
        desc.base_color[0].abs() * color[0] * brightness,
        desc.base_color[1].abs() * color[1] * brightness,
        desc.base_color[2].abs() * color[2] * brightness,
    ]
}

/// Exact maximum of the runtime Catmull-Rom scalar evaluator, after its
/// non-negative clamp.  It checks segment endpoints and every derivative root
/// in the segment interior, so authored key samples alone cannot miss an
/// overshoot.
pub fn curve_nonnegative_supremum(samples: &[f32], fallback: f32, open: bool) -> f32 {
    if samples.is_empty() {
        return fallback.max(0.0);
    }
    if samples.len() == 1 {
        return samples[0].max(0.0);
    }
    curve_supremum(samples.len(), open, |index| samples[index]).max(0.0)
}

fn curve_nonnegative_supremum_rgb(samples: &[[f32; 3]], channel: usize, open: bool) -> f32 {
    curve_supremum(samples.len(), open, |index| samples[index][channel]).max(0.0)
}

fn curve_supremum(count: usize, open: bool, value: impl Fn(usize) -> f32) -> f32 {
    debug_assert!(count >= 2);
    let segment_count = if open { count - 1 } else { count };
    let mut maximum = f32::NEG_INFINITY;
    for segment in 0..segment_count {
        let (i0, i1, i2, i3) = if open {
            (
                segment.saturating_sub(1),
                segment,
                (segment + 1).min(count - 1),
                (segment + 2).min(count - 1),
            )
        } else {
            (
                (segment + count - 1) % count,
                segment,
                (segment + 1) % count,
                (segment + 2) % count,
            )
        };
        let p0 = value(i0);
        let p1 = value(i1);
        let p2 = value(i2);
        let p3 = value(i3);
        let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
        let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c = -0.5 * p0 + 0.5 * p2;
        maximum = maximum.max(p1).max(((a + b) + c) + p1);
        // The derivative is 3a*f² + 2b*f + c.  Degenerate linear cases are
        // handled directly; duplicate roots are harmless.
        let qa = 3.0 * a;
        let qb = 2.0 * b;
        if qa.abs() <= f32::EPSILON {
            if qb.abs() > f32::EPSILON {
                let f = -c / qb;
                if (0.0..1.0).contains(&f) {
                    maximum = maximum.max(((a * f + b) * f + c) * f + p1);
                }
            }
        } else {
            let discriminant = qb * qb - 4.0 * qa * c;
            if discriminant >= 0.0 {
                let root = discriminant.sqrt();
                for f in [(-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa)] {
                    if (0.0..1.0).contains(&f) {
                        maximum = maximum.max(((a * f + b) * f + c) * f + p1);
                    }
                }
            }
        }
    }
    maximum
}

/// Exact runtime output difference for one direct channel.  This is used by
/// tests to pin the signed subtraction, clamp, and f16 storage semantics.  The
/// zero-only acceptance rule below has `delta == 0`, making this zero for every
/// `weight ∈ [0, 1]`, including clamp transitions.
pub fn direct_f16_output_error(base: f32, delta: f32, weight: f32) -> f32 {
    let weight = weight.clamp(0.0, 1.0);
    let before = f16_bits_to_f32(f32_to_f16_bits(base.max(0.0)));
    let after = f16_bits_to_f32(f32_to_f16_bits((base - delta * weight).max(0.0)));
    (before - after).abs()
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
            affinity_offsets: vec![0, 2, lights.len() as u32],
            affinity_lights: lights,
            delta_subblocks: payload,
        }
    }

    fn light(animated: bool, dynamic: bool) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
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

    #[test]
    fn exact_zero_drop_is_safe_for_signed_clamped_f16_direct_compose() {
        for weight in [0.0, 0.37, 1.0] {
            assert_eq!(direct_f16_output_error(0.0002, 0.0, weight), 0.0);
            assert_eq!(direct_f16_output_error(1.0, 0.0, weight), 0.0);
        }
        // The interior case crosses the clamp boundary for a nonzero delta;
        // keeping it demonstrates why only exact zero is accepted today.
        assert!(direct_f16_output_error(0.003, 0.01, 0.37) > MAX_OUTPUT_ERROR);
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
    fn authored_curve_bound_includes_internal_catmull_rom_overshoot() {
        // The keys top out at 1, while this exact closed Catmull-Rom segment
        // has a positive internal overshoot.
        let desc = AnimationDescriptor {
            base_color: [2.0, 1.0, 1.0],
            brightness: vec![0.0, 1.0, 1.0, 0.0],
            color: vec![[1.0; 3]; 4],
            ..AnimationDescriptor::default()
        };
        let bound = immutable_descriptor_scale_bound(&desc);
        assert!(
            bound[0] > 2.0,
            "internal extrema must exceed authored samples"
        );
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
