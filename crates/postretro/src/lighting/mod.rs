// Direct lighting CPU data: convert MapLight records into the packed
// GpuLight byte layout consumed by the forward pass storage buffer.
//
// See: context/lib/rendering_pipeline.md §4

pub mod chunk_list;
pub(crate) mod cone_frustum;
pub mod influence;
pub mod lightmap;
pub mod spec_buffer;
pub mod spot_shadow;

use crate::prl::{FalloffModel, LightType, MapLight};

/// On-disk size of a single `GpuLight` record in the storage buffer.
///
/// Layout matches the WGSL `GpuLight` struct in `forward.wgsl` — four
/// `vec4<f32>` slots in order:
///   0: position_and_type        (xyz = world position, w = bitcast<f32>(light_type))
///   1: color_and_falloff_model  (xyz = linear RGB × intensity, w = bitcast<f32>(falloff_model))
///   2: direction_and_range      (xyz = aim direction, w = falloff_range meters)
///   3: cone_angles_and_pad      (x = inner angle rad, y = outer angle rad, z = shadow-slot index (bitcast u32), w = pad)
///
/// Each vec4<f32> is 16 bytes; the struct has only vec4 members so its
/// alignment is 16 and the array stride is an exact multiple of 16.
pub const GPU_LIGHT_SIZE: usize = 64;

/// Byte offset of the shadow-slot index within a packed `GpuLight`
/// (`cone_angles_and_pad.z`). Exposed so the renderer can patch just this
/// field into an already-packed light buffer without re-packing the whole
/// record — the animated light bridge owns the base bytes, the shadow pool
/// owns this slot field, and a full re-pack from either side would clobber
/// the other.
pub const SHADOW_SLOT_BYTE_OFFSET: usize = 56;

/// Whether a runtime light renders animated ENTITY meshes as occluders into its
/// shadow slot. The single shared predicate for the entity-occluder gate, called
/// by the spot path (now) and the future cube path (Task 5).
///
/// This is the SECOND of two separate gates (see
/// `context/lib/rendering_pipeline.md` §7.1): pool-*slot* eligibility (does the
/// light get a shadow map for its WORLD shadow) is `is_dynamic` in
/// `SpotShadowPool::rank_lights`; entity-*occluder* rendering into that slot is
/// this gate. A dynamic light with `casts_entity_shadows` off still casts its
/// world shadow but draws no entity occluders, so the two gates must not be
/// conflated.
///
/// `casts_entity_shadows && is_dynamic`: only `is_dynamic` lights (the
/// `light_dynamic`/`light_dynamic_spot` classnames) cast crisp runtime entity
/// shadows, and the per-light `casts_entity_shadows` toggle opts that in.
pub fn entity_occluder_eligible(light: &MapLight) -> bool {
    light.casts_entity_shadows && light.is_dynamic
}

/// Encode the `LightType` discriminant the way the shader expects it:
/// a `u32` bit-cast into the `w` slot of the `position_and_type` vec4.
fn light_type_u32(ty: LightType) -> u32 {
    match ty {
        LightType::Point => 0,
        LightType::Spot => 1,
        LightType::Directional => 2,
    }
}

/// Encode the `FalloffModel` discriminant the same way for the shader.
fn falloff_model_u32(fm: FalloffModel) -> u32 {
    match fm {
        FalloffModel::Linear => 0,
        FalloffModel::InverseDistance => 1,
        FalloffModel::InverseSquared => 2,
    }
}

/// Pack one `MapLight` into the shader-facing `GpuLight` byte layout.
///
/// Pre-multiplies `color × intensity` on the CPU so the shader does one
/// mul per light in the inner loop instead of two. For `Directional` and
/// `Point` lights the unused fields (direction for Point, cone angles for
/// non-Spot, position for Directional in the shader's logic) still need
/// to be zeroed to keep the record deterministic.
///
/// The `slot_index` is written to bytes 56–59 (cone_angles_and_pad z component).
/// Sentinel `0xFFFFFFFF` = no shadow slot allocated.
pub fn pack_light_with_slot(light: &MapLight, slot_index: u32) -> [u8; GPU_LIGHT_SIZE] {
    let mut bytes = [0u8; GPU_LIGHT_SIZE];

    // slot 0: position_and_type — world position (f32x3) + bitcast<f32>(u32 light_type)
    let px = light.origin[0] as f32;
    let py = light.origin[1] as f32;
    let pz = light.origin[2] as f32;
    let type_bits = light_type_u32(light.light_type);
    write_f32(&mut bytes, 0, px);
    write_f32(&mut bytes, 4, py);
    write_f32(&mut bytes, 8, pz);
    write_u32_as_f32(&mut bytes, 12, type_bits);

    // slot 1: color_and_falloff_model — color × intensity + bitcast<f32>(u32 falloff_model)
    let cr = light.color[0] * light.intensity;
    let cg = light.color[1] * light.intensity;
    let cb = light.color[2] * light.intensity;
    let falloff_bits = falloff_model_u32(light.falloff_model);
    write_f32(&mut bytes, 16, cr);
    write_f32(&mut bytes, 20, cg);
    write_f32(&mut bytes, 24, cb);
    write_u32_as_f32(&mut bytes, 28, falloff_bits);

    // slot 2: direction_and_range — cone aim direction + falloff_range
    // cone_direction is already the aim direction (light → target) per MapLight;
    // we store it as-is. Point lights may have [0,0,0] here; the shader
    // branches on light_type so the zero won't be consumed.
    write_f32(&mut bytes, 32, light.cone_direction[0]);
    write_f32(&mut bytes, 36, light.cone_direction[1]);
    write_f32(&mut bytes, 40, light.cone_direction[2]);
    write_f32(&mut bytes, 44, light.falloff_range);

    // slot 3: cone_angles_and_pad — inner + outer cone angles (radians) + shadow slot index
    write_f32(&mut bytes, 48, light.cone_angle_inner);
    write_f32(&mut bytes, 52, light.cone_angle_outer);
    // bytes 56..60 hold the shadow slot index (0..8 or 0xFFFFFFFF for no slot).
    // Shader reads as f32 then bitcasts back to u32; round-trip preserves bit patterns.
    write_u32_as_f32(&mut bytes, SHADOW_SLOT_BYTE_OFFSET, slot_index);
    // bytes 60..64 stay zero — reserved pad.

    bytes
}

/// Legacy wrapper for backward compatibility. Calls `pack_light_with_slot` with `NO_SHADOW_SLOT`.
pub fn pack_light(light: &MapLight) -> [u8; GPU_LIGHT_SIZE] {
    pack_light_with_slot(light, crate::lighting::spot_shadow::NO_SHADOW_SLOT)
}

/// Pack the full light list into a contiguous byte buffer suitable for
/// `queue.write_buffer` into the shader's `array<GpuLight>` binding.
///
/// Each light's shadow slot index is set to `NO_SHADOW_SLOT` (no shadow).
/// For runtime slot allocation, use `pack_lights_with_slots` instead.
pub fn pack_lights(lights: &[MapLight]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lights.len() * GPU_LIGHT_SIZE);
    for light in lights {
        bytes.extend_from_slice(&pack_light(light));
    }
    bytes
}

/// Pack the full light list with per-light shadow slot assignments into `bytes`,
/// reusing the allocation. Clears the buffer first so the caller can memcompare
/// against a previous frame's bytes to skip a redundant `queue.write_buffer`.
///
/// `slot_indices` must have the same length as `lights`.
/// Each entry is a slot index (0..8) or `NO_SHADOW_SLOT` for unshadowed.
pub fn pack_lights_with_slots_into(bytes: &mut Vec<u8>, lights: &[MapLight], slot_indices: &[u32]) {
    debug_assert_eq!(
        lights.len(),
        slot_indices.len(),
        "lights and slot_indices must be the same length"
    );
    bytes.clear();
    // Reserve after clear so growth only happens when the light count increases;
    // if the Vec already has enough capacity this is a no-op.
    bytes.reserve(lights.len() * GPU_LIGHT_SIZE);
    for (light, &slot) in lights.iter().zip(slot_indices.iter()) {
        bytes.extend_from_slice(&pack_light_with_slot(light, slot));
    }
}

/// Overwrite only the shadow-slot field of each already-packed light in
/// `buffer` with the corresponding entry in `slots`, leaving every other byte
/// (the animated base data the light bridge owns) untouched. Returns `true` if
/// any byte changed, so the caller can skip a redundant GPU upload.
///
/// This is the seam that lets the animated-light bridge and the shadow pool
/// share one GPU light buffer: the bridge packs base records (with sentinel
/// slots) and the pool patches the live slot here. Re-packing the whole record
/// from either side would clobber the other. `buffer` must already hold
/// `slots.len()` packed `GpuLight` records.
pub fn patch_shadow_slots(buffer: &mut [u8], slots: &[u32]) -> bool {
    debug_assert!(buffer.len() >= slots.len() * GPU_LIGHT_SIZE);
    let mut changed = false;
    for (i, &slot) in slots.iter().enumerate() {
        let off = i * GPU_LIGHT_SIZE + SHADOW_SLOT_BYTE_OFFSET;
        // Guard the slice in release builds too; a mismatch between buffer size
        // and slots length should be caught by the debug_assert above, but skip
        // rather than panic or corrupt a neighbour record if it slips through.
        let Some(field) = buffer.get_mut(off..off + 4) else {
            continue;
        };
        let new = slot.to_ne_bytes();
        if *field != new {
            field.copy_from_slice(&new);
            changed = true;
        }
    }
    changed
}

#[inline]
fn write_f32(dst: &mut [u8], offset: usize, value: f32) {
    dst[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

/// Write a `u32` into the slot, reusing the same 4 bytes a `f32` would
/// occupy. The round-trip is safe because the shader reads the field with
/// `bitcast<u32>(...)`, recovering the original integer bit-for-bit.
/// Using `to_ne_bytes` matches `write_f32` so both keep the same native
/// byte order in the packed record.
#[inline]
fn write_u32_as_f32(dst: &mut [u8], offset: usize, value: u32) {
    dst[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_point() -> MapLight {
        MapLight {
            origin: [1.0, 2.0, 3.0],
            light_type: LightType::Point,
            intensity: 2.0,
            color: [0.25, 0.5, 1.0],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: 12.5,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

    fn sample_spot() -> MapLight {
        MapLight {
            origin: [-4.0, 1.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.5,
            color: [1.0, 0.8, 0.6],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.5,
            cone_angle_outer: 0.8,
            cone_direction: [0.0, -1.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

    fn sample_directional() -> MapLight {
        MapLight {
            origin: [0.0, 100.0, 0.0],
            light_type: LightType::Directional,
            intensity: 0.9,
            color: [0.8, 0.9, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 0.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, -1.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

    fn read_f32(src: &[u8], offset: usize) -> f32 {
        f32::from_ne_bytes(src[offset..offset + 4].try_into().unwrap())
    }

    fn read_u32(src: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(src[offset..offset + 4].try_into().unwrap())
    }

    /// The entity-occluder gate is `casts_entity_shadows && is_dynamic`: a
    /// dynamic light with the toggle on draws entity occluders; the toggle off,
    /// or a non-dynamic light, draws none (it may still cast a world shadow).
    /// This is the predicate Task 3 builds the FGD/compiler semantics around.
    #[test]
    fn entity_occluder_gate_requires_dynamic_and_toggle() {
        let mut light = sample_spot();

        // Dynamic + toggle on → eligible.
        light.is_dynamic = true;
        light.casts_entity_shadows = true;
        assert!(
            entity_occluder_eligible(&light),
            "dynamic light with casts_entity_shadows on must render entity occluders"
        );

        // Dynamic + toggle off → not eligible (world shadow only).
        light.casts_entity_shadows = false;
        assert!(
            !entity_occluder_eligible(&light),
            "dynamic light with the toggle off casts no entity shadow"
        );

        // Non-dynamic + toggle on → not eligible (only is_dynamic lights cast
        // crisp runtime entity shadows).
        light.is_dynamic = false;
        light.casts_entity_shadows = true;
        assert!(
            !entity_occluder_eligible(&light),
            "a non-dynamic light never renders entity occluders, even toggled on"
        );
    }

    #[test]
    fn gpu_light_size_matches_expected_stride() {
        // 4 vec4<f32> slots × 16 bytes each.
        assert_eq!(GPU_LIGHT_SIZE, 64);
    }

    #[test]
    fn pack_point_light_encodes_position_color_type() {
        let light = sample_point();
        let bytes = pack_light(&light);

        assert_eq!(read_f32(&bytes, 0), 1.0);
        assert_eq!(read_f32(&bytes, 4), 2.0);
        assert_eq!(read_f32(&bytes, 8), 3.0);
        assert_eq!(read_u32(&bytes, 12), 0); // Point

        // color × intensity is pre-multiplied on the CPU.
        assert!((read_f32(&bytes, 16) - 0.5).abs() < 1e-6);
        assert!((read_f32(&bytes, 20) - 1.0).abs() < 1e-6);
        assert!((read_f32(&bytes, 24) - 2.0).abs() < 1e-6);
        assert_eq!(read_u32(&bytes, 28), 2); // InverseSquared

        // range lives in slot 2 w.
        assert_eq!(read_f32(&bytes, 44), 12.5);

        // slot index should be NO_SHADOW_SLOT.
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
    }

    #[test]
    fn pack_spot_light_encodes_direction_and_cone_angles() {
        let light = sample_spot();
        let bytes = pack_light(&light);

        assert_eq!(read_u32(&bytes, 12), 1); // Spot
        assert_eq!(read_u32(&bytes, 28), 0); // Linear

        // direction (slot 2 xyz)
        assert_eq!(read_f32(&bytes, 32), 0.0);
        assert_eq!(read_f32(&bytes, 36), -1.0);
        assert_eq!(read_f32(&bytes, 40), 0.0);
        assert_eq!(read_f32(&bytes, 44), 20.0);

        // cone angles (slot 3 x/y)
        assert_eq!(read_f32(&bytes, 48), 0.5);
        assert_eq!(read_f32(&bytes, 52), 0.8);
    }

    #[test]
    fn pack_directional_light_has_zero_range_and_type_2() {
        let light = sample_directional();
        let bytes = pack_light(&light);

        assert_eq!(read_u32(&bytes, 12), 2); // Directional
        assert_eq!(read_f32(&bytes, 44), 0.0); // no range for directional
        // direction is still stored so the shader can compute -L.
        assert_eq!(read_f32(&bytes, 36), -1.0);
    }

    #[test]
    fn pack_lights_empty_list_is_empty_buffer() {
        let bytes = pack_lights(&[]);
        assert!(bytes.is_empty());
    }

    #[test]
    fn pack_lights_concatenates_records() {
        let lights = vec![sample_point(), sample_spot(), sample_directional()];
        let bytes = pack_lights(&lights);
        assert_eq!(bytes.len(), 3 * GPU_LIGHT_SIZE);

        // First record's type should still be Point.
        assert_eq!(read_u32(&bytes, 12), 0);
        // Second record's type should be Spot.
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + 12), 1);
        // Third record's type should be Directional.
        assert_eq!(read_u32(&bytes, 2 * GPU_LIGHT_SIZE + 12), 2);
    }

    #[test]
    fn cone_pad_is_zeroed_for_point_light() {
        let bytes = pack_light(&sample_point());
        // cone_angles_and_pad slot, z = shadow slot index, w = pad = bytes 60..64
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
        for &b in &bytes[60..64] {
            assert_eq!(b, 0);
        }
    }

    #[test]
    fn pack_lights_with_slots_encodes_slot_indices() {
        let lights = vec![sample_point(), sample_spot()];
        let slots = vec![0u32, 1u32];
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &slots);

        // First light at slot 0.
        assert_eq!(read_u32(&bytes, 56), 0);
        // Second light at slot 1.
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + 56), 1);
    }

    #[test]
    fn pack_lights_with_slots_no_slot_sentinel() {
        let lights = vec![sample_point(), sample_spot()];
        let slots = vec![crate::lighting::spot_shadow::NO_SHADOW_SLOT, 2u32];
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &slots);

        // First light unshadowed.
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
        // Second light at slot 2.
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + 56), 2);
    }

    /// Regression: the animated light bridge packs base records with sentinel
    /// shadow slots; the shadow pool must patch the live slot onto that buffer
    /// without disturbing the base data, or the bridge's sentinel persists and
    /// the forward shader never samples the shadow map.
    #[test]
    fn patch_shadow_slots_sets_slot_and_preserves_base_bytes() {
        let lights = vec![sample_point(), sample_spot()];
        // Start as the bridge would: every slot the sentinel.
        let sentinel = crate::lighting::spot_shadow::NO_SHADOW_SLOT;
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &[sentinel, sentinel]);
        let base_before = bytes.clone();

        // Pool assigns the second light to slot 1.
        let changed = patch_shadow_slots(&mut bytes, &[sentinel, 1u32]);
        assert!(
            changed,
            "patching a sentinel to a real slot must report change"
        );
        assert_eq!(
            read_u32(&bytes, 56),
            sentinel,
            "first light stays unshadowed"
        );
        assert_eq!(
            read_u32(&bytes, GPU_LIGHT_SIZE + 56),
            1,
            "second light gets slot 1"
        );

        // Every byte except the second light's slot field is untouched.
        for (i, (a, b)) in base_before.iter().zip(bytes.iter()).enumerate() {
            let in_patched_slot = (GPU_LIGHT_SIZE + SHADOW_SLOT_BYTE_OFFSET
                ..GPU_LIGHT_SIZE + SHADOW_SLOT_BYTE_OFFSET + 4)
                .contains(&i);
            if !in_patched_slot {
                assert_eq!(a, b, "base byte {i} must be preserved");
            }
        }
    }

    #[test]
    fn patch_shadow_slots_no_change_reports_false() {
        let lights = vec![sample_point(), sample_spot()];
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &[3u32, 1u32]);
        // Patching the same slots back is a no-op — caller skips the GPU upload.
        assert!(!patch_shadow_slots(&mut bytes, &[3u32, 1u32]));
    }

    #[test]
    fn pack_lights_with_slots_into_identical_inputs_produce_byte_equal_outputs() {
        let lights = vec![sample_point(), sample_spot(), sample_directional()];
        let slots = vec![0u32, 1u32, crate::lighting::spot_shadow::NO_SHADOW_SLOT];

        let mut first = Vec::new();
        pack_lights_with_slots_into(&mut first, &lights, &slots);
        let snapshot = first.clone();

        let mut second = Vec::new();
        pack_lights_with_slots_into(&mut second, &lights, &slots);

        assert_eq!(snapshot, second);
    }

    #[test]
    fn pack_lights_with_slots_into_clears_prepopulated_buffer_before_packing() {
        let lights = vec![sample_point()];
        let slots = vec![0u32];

        // Pre-fill with garbage so we can confirm the output is clean.
        let mut bytes = vec![0xFFu8; 512];
        pack_lights_with_slots_into(&mut bytes, &lights, &slots);

        assert_eq!(bytes.len(), GPU_LIGHT_SIZE);
        // The first record's type field should be Point (0), not the garbage 0xFF pattern.
        assert_eq!(read_u32(&bytes, 12), 0);
    }

    #[test]
    fn pack_lights_with_slots_into_shorter_list_produces_shorter_buffer() {
        let lights_two = vec![sample_point(), sample_spot()];
        let slots_two = vec![0u32, 1u32];
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights_two, &slots_two);
        let len_two = bytes.len();

        let lights_one = vec![sample_point()];
        let slots_one = vec![0u32];
        pack_lights_with_slots_into(&mut bytes, &lights_one, &slots_one);

        // A shorter list must produce a shorter buffer so the byte-comparison
        // in the renderer detects the change and issues a write_buffer.
        assert!(bytes.len() < len_two);
        assert_eq!(bytes.len(), GPU_LIGHT_SIZE);
    }
}
