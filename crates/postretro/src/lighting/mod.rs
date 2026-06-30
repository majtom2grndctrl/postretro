// Direct lighting CPU data: convert MapLight records into the packed
// GpuLight byte layout consumed by the forward pass storage buffer.
//
// See: context/lib/rendering_pipeline.md §4

pub mod chunk_list;
pub mod cone_frustum;
pub mod cube_shadow;
pub mod influence;
pub mod lightmap;
pub(crate) mod script_primitives;
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
///   3: cone_angles_and_pad      (x = inner angle rad, y = outer angle rad, z = spot shadow-slot index (bitcast u32), w = cube point shadow-slot index (bitcast u32))
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

/// Byte offset of the dynamic POINT-light cube-shadow slot within a packed
/// `GpuLight` (`cone_angles_and_pad.w`, bytes 60..64 — the former reserved pad).
/// Patched independently of the spot slot (which rides `.z` at byte 56): a point
/// light reads ONLY `.w` for its cube slot, a spot reads ONLY `.z`, so the two
/// shadow paths never alias. Sentinel `0xFFFFFFFF` = not ranked into the cube
/// pool (the forward point case then does unshadowed attenuation).
pub const CUBE_SLOT_BYTE_OFFSET: usize = 60;

/// Whether a runtime light renders animated ENTITY meshes as occluders into its
/// shadow slot. The single shared predicate for the entity-occluder gate, called
/// by both the spot path and the cube (point-light) path.
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

/// Shadow-slot eligibility predicate: does this light's influence volume reach
/// any cell the camera can pull geometry from this frame?
///
/// A dynamic light is shadow-eligible when its influence sphere (`origin`,
/// radius `falloff_range`) overlaps any AABB in `reachable_cell_aabbs`. That set
/// is the AABBs of the **fog/light-reachable cells** — the *wider*
/// portal-reachable set (the same one behind `light_reachable_cell_mask`), which
/// deliberately INCLUDES empty `face_count == 0` cells (see
/// `visibility.rs`). It is NOT the narrower `VisibleCells` drawable set; using
/// the wider set is intentional, because we are bounding light *influence* (an
/// empty reachable cell still bounds it) — narrowing it to the drawable set
/// would re-drop lights in empty reachable cells and reintroduce a variant of
/// the bug below.
///
/// This is the same principle the WORLD occluder cull already uses
/// (`shadow_cull.rs`): a shadow caster — here the LIGHT — does not need its OWN
/// cell to be in the camera PVS; it only needs to be able to reach a receiver
/// the camera sees.
///
/// Replaces the prior over-strict gate that tested whether the light's own cell
/// was in the portal-reachable set. That gate dropped a light whose own cell was
/// occluded from the camera even though the light still illuminated (and
/// shadowed) geometry directly in view, so entity shadows vanished as the camera
/// pitched down and the light's cell left the shrinking PVS.
///
/// Still a real cull: a light whose influence sphere reaches NONE of the
/// reachable cells returns `false` (distant lights that cannot affect the view
/// are skipped), so eligibility is not made unconditional.
///
/// `reachable_cell_aabbs` empty = DrawAll sentinel (fallback visibility paths) →
/// always eligible, matching the empty-mask DrawAll contract the caller relies
/// on. The test is a cheap sphere-vs-AABB squared-distance check per cell with
/// an early-out on the first hit.
pub fn light_reaches_visible_cell(
    origin: glam::Vec3,
    falloff_range: f32,
    reachable_cell_aabbs: &[(glam::Vec3, glam::Vec3)],
) -> bool {
    // DrawAll sentinel: no per-cell set supplied → keep every light eligible.
    if reachable_cell_aabbs.is_empty() {
        return true;
    }
    let r = falloff_range.max(0.0);
    let r_sq = r * r;
    reachable_cell_aabbs.iter().any(|(min, max)| {
        // Closest point on the AABB to the light origin, then squared distance.
        let closest = origin.clamp(*min, *max);
        closest.distance_squared(origin) <= r_sq
    })
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
    // bytes 56..60 hold the shadow slot index (0..SHADOW_POOL_SIZE or 0xFFFFFFFF for no slot).
    // Shader reads as f32 then bitcasts back to u32; round-trip preserves bit patterns.
    write_u32_as_f32(&mut bytes, SHADOW_SLOT_BYTE_OFFSET, slot_index);
    // bytes 60..64 hold the cube (point) shadow slot. Default to the sentinel so
    // an un-ranked point light does unshadowed attenuation; the cube ranker
    // patches the live slot via `patch_cube_slots` after this base pack.
    write_u32_as_f32(
        &mut bytes,
        CUBE_SLOT_BYTE_OFFSET,
        crate::lighting::spot_shadow::NO_SHADOW_SLOT,
    );

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
/// Each entry is a slot index (0..SHADOW_POOL_SIZE) or `NO_SHADOW_SLOT` for unshadowed.
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

/// Overwrite only the cube (point) shadow-slot field of each already-packed
/// light in `buffer` with the corresponding entry in `slots`, leaving every
/// other byte (the spot slot and the animated base data) untouched. Returns
/// `true` if any byte changed, so the caller can skip a redundant GPU upload.
///
/// The cube and spot slot fields live in disjoint bytes of the same
/// `cone_angles_and_pad` row (cube `.w` = bytes 60..64, spot `.z` = bytes
/// 56..60), so `patch_cube_slots` and `patch_shadow_slots` compose without
/// clobbering each other. `buffer` must already hold `slots.len()` packed
/// `GpuLight` records.
pub fn patch_cube_slots(buffer: &mut [u8], slots: &[u32]) -> bool {
    debug_assert!(buffer.len() >= slots.len() * GPU_LIGHT_SIZE);
    let mut changed = false;
    for (i, &slot) in slots.iter().enumerate() {
        let off = i * GPU_LIGHT_SIZE + CUBE_SLOT_BYTE_OFFSET;
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
            cell_index: 0,
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
            cell_index: 0,
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
            cell_index: 0,
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
    fn cone_slots_default_to_sentinel_for_point_light() {
        let bytes = pack_light(&sample_point());
        // cone_angles_and_pad: z (byte 56) = spot slot, w (byte 60) = cube slot.
        // Both default to the no-slot sentinel for an un-ranked light.
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
        assert_eq!(
            read_u32(&bytes, CUBE_SLOT_BYTE_OFFSET),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
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

    /// A freshly packed light defaults its cube (point) slot to the sentinel, so
    /// an un-ranked point light does unshadowed attenuation in the forward pass.
    #[test]
    fn pack_defaults_cube_slot_to_sentinel() {
        let bytes = pack_light_with_slot(&sample_point(), 0u32);
        assert_eq!(
            read_u32(&bytes, CUBE_SLOT_BYTE_OFFSET),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT,
            "cube slot must default to the sentinel"
        );
    }

    /// `patch_cube_slots` writes only the cube slot field (`.w`, byte 60) and
    /// leaves every other byte untouched.
    #[test]
    fn patch_cube_slots_sets_slot_and_preserves_base_bytes() {
        let lights = vec![sample_point(), sample_point()];
        let sentinel = crate::lighting::spot_shadow::NO_SHADOW_SLOT;
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &[sentinel, sentinel]);
        let base_before = bytes.clone();

        let changed = patch_cube_slots(&mut bytes, &[sentinel, 2u32]);
        assert!(
            changed,
            "patching a sentinel to a real cube slot reports change"
        );
        assert_eq!(read_u32(&bytes, CUBE_SLOT_BYTE_OFFSET), sentinel);
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + CUBE_SLOT_BYTE_OFFSET), 2);

        for (i, (a, b)) in base_before.iter().zip(bytes.iter()).enumerate() {
            let in_patched = (GPU_LIGHT_SIZE + CUBE_SLOT_BYTE_OFFSET
                ..GPU_LIGHT_SIZE + CUBE_SLOT_BYTE_OFFSET + 4)
                .contains(&i);
            if !in_patched {
                assert_eq!(a, b, "base byte {i} must be preserved");
            }
        }
    }

    /// The spot slot (`.z`, byte 56) and cube slot (`.w`, byte 60) live in
    /// disjoint bytes of the same row, so the two patches compose without one
    /// clobbering the other — a single light can carry both a spot and a cube
    /// slot independently (the shader reads only the field for its light type).
    #[test]
    fn patch_spot_and_cube_slots_are_disjoint() {
        let lights = vec![sample_point()];
        let sentinel = crate::lighting::spot_shadow::NO_SHADOW_SLOT;
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &[sentinel]);

        patch_shadow_slots(&mut bytes, &[7u32]);
        patch_cube_slots(&mut bytes, &[3u32]);

        assert_eq!(
            read_u32(&bytes, SHADOW_SLOT_BYTE_OFFSET),
            7,
            "spot slot intact"
        );
        assert_eq!(
            read_u32(&bytes, CUBE_SLOT_BYTE_OFFSET),
            3,
            "cube slot intact"
        );
    }

    #[test]
    fn patch_cube_slots_no_change_reports_false() {
        let lights = vec![sample_point(), sample_point()];
        let mut bytes = Vec::new();
        pack_lights_with_slots_into(&mut bytes, &lights, &[0u32, 0u32]);
        patch_cube_slots(&mut bytes, &[4u32, 1u32]);
        // Patching the same cube slots back is a no-op.
        assert!(!patch_cube_slots(&mut bytes, &[4u32, 1u32]));
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

    // --- Shadow-slot eligibility: influence reaches visible cell --------------

    use glam::Vec3;

    /// Regression: light denied shadow slot because its own leaf left the camera
    /// PVS, even though its light reaches visible receivers (shadows vanished on
    /// pitch-down).
    ///
    /// The light sits inside a cell the camera can NO LONGER see (its own leaf is
    /// not among `visible_aabbs`), but its influence sphere still overlaps a cell
    /// the camera DOES see — the receiver geometry directly in view. Eligibility
    /// must track that reachable receiver, not the light's own-leaf PVS
    /// membership.
    #[test]
    fn light_with_off_pvs_leaf_but_reachable_receiver_is_eligible() {
        // Camera-visible cell: a small room near the origin.
        let visible_aabbs = vec![(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))];
        // Light sits at x=-10 — OUTSIDE the visible cell (its own occluded leaf),
        // but with a 10 m influence range its sphere reaches into the visible
        // cell (nearest visible-cell point is x=-5, distance 5 < 10).
        let origin = Vec3::new(-10.0, 2.0, 0.0);
        let falloff_range = 10.0;
        assert!(
            light_reaches_visible_cell(origin, falloff_range, &visible_aabbs),
            "a light whose influence reaches a visible cell must be shadow-eligible \
             even when its own leaf is off the camera PVS"
        );
    }

    /// Guard (the fix must not make eligibility unconditional): a light whose
    /// influence sphere reaches NONE of the visible cells is still correctly
    /// skipped, so a genuine distance cull survives.
    #[test]
    fn light_too_far_from_every_visible_cell_is_not_eligible() {
        let visible_aabbs = vec![(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))];
        // Light 100 m away with only a 10 m range cannot reach the visible cell
        // (nearest visible-cell point is x=5, distance 95 > 10).
        let origin = Vec3::new(100.0, 2.0, 0.0);
        let falloff_range = 10.0;
        assert!(
            !light_reaches_visible_cell(origin, falloff_range, &visible_aabbs),
            "a light whose influence cannot reach any visible cell must be skipped"
        );
    }

    /// Orientation/PVS-invariance property: for a fixed light whose influence
    /// reaches a fixed visible receiver cell, eligibility is invariant as the
    /// camera-reachable leaf set shrinks or grows around that receiver. This pins
    /// the symptom directly — the shadow vanished purely because the PVS shrank
    /// on pitch-down, dropping the light's own leaf.
    #[test]
    fn eligibility_invariant_as_pvs_set_shrinks_around_fixed_receiver() {
        // The fixed receiver cell the camera always sees and the light reaches.
        let receiver = (Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0));
        // The light's own cell — far off to the side, which the PVS may or may
        // not include depending on camera pitch.
        let light_own_cell = (Vec3::new(-20.0, 0.0, -5.0), Vec3::new(-12.0, 4.0, 5.0));
        // Other distant cells that drift in and out of the PVS as the camera
        // sweeps; none of these change whether the light reaches `receiver`.
        let distant_a = (Vec3::new(200.0, 0.0, 0.0), Vec3::new(210.0, 4.0, 10.0));
        let distant_b = (Vec3::new(-200.0, 0.0, 0.0), Vec3::new(-190.0, 4.0, 10.0));

        let origin = Vec3::new(-10.0, 2.0, 0.0);
        let falloff_range = 10.0; // reaches `receiver` (nearest point x=-5, d=5).

        // Sweep a family of PVS sets that all CONTAIN the fixed receiver but vary
        // in which other cells (including the light's own cell) are reachable.
        let pvs_variants: Vec<Vec<(Vec3, Vec3)>> = vec![
            vec![receiver],
            vec![receiver, light_own_cell],
            vec![receiver, distant_a],
            vec![receiver, distant_b, distant_a],
            vec![receiver, light_own_cell, distant_a, distant_b],
        ];
        for pvs in &pvs_variants {
            assert!(
                light_reaches_visible_cell(origin, falloff_range, pvs),
                "eligibility must hold for any PVS set containing the reachable receiver, \
                 regardless of whether the light's own cell is in the set"
            );
        }
    }

    /// Empty `visible_aabbs` is the DrawAll sentinel (fallback visibility paths)
    /// — every light stays eligible, matching the caller's empty-mask contract.
    #[test]
    fn empty_visible_set_treats_every_light_as_eligible() {
        assert!(
            light_reaches_visible_cell(Vec3::new(9999.0, 9999.0, 9999.0), 0.1, &[]),
            "empty visible set = DrawAll sentinel → light stays eligible"
        );
    }
}
