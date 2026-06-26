// Per-frame fog-volume cell-mask derivation and sphere/fog-AABB intersection.
// See: context/lib/rendering_pipeline.md

use super::*;

/// Derives the per-frame active fog-volume bitmask from the wider
/// fog-reachable cell set produced by portal traversal.
///
/// - `fog_reachable` non-empty + masks present: OR each reachable cell's mask.
/// - `fog_reachable` empty: portal isolation doesn't apply — empty world,
///   solid-cell camera, exterior camera, or no-portals map. Every canonical
///   slot stays active.
/// - `fog_reachable` non-empty + masks absent: caller supplied no mask table.
///   Keep all canonical slots active. Modern PRL loading rejects missing
///   `FogCellMasks` when canonical fog volumes exist.
///
/// `camera_cell`'s own fog mask bits are always unioned into the result when
/// masks are present, regardless of whether the camera cell appears in
/// `fog_reachable`. Portal traversal can omit the camera cell on transient
/// frames (e.g., grazing a portal seam); unioning prevents fog the camera is
/// inside from flickering off. Idempotent when the camera cell is already in
/// `fog_reachable`.
///
/// Must be called after `FogPass::set_canonical_volumes`; before = 0
/// canonical count = 0 mask.
pub(crate) fn compute_fog_cell_mask(
    fog_reachable: &[u32],
    fog_cell_masks: Option<&[u32]>,
    canonical_volume_count: u32,
    camera_cell: Option<u32>,
) -> u32 {
    let all_slots_mask = if canonical_volume_count >= 32 {
        u32::MAX
    } else {
        (1u32 << canonical_volume_count).wrapping_sub(1)
    };
    match (fog_reachable.is_empty(), fog_cell_masks) {
        // Empty fog_reachable: portal isolation doesn't apply — either the world is
        // empty (DrawAll arm), or a non-portal fallback ran (solid-cell, exterior,
        // no-portals) and produced no fog_reachable set. All canonical slots active.
        (true, _) => all_slots_mask,
        // AND against `all_slots_mask` so reserved bits 16..32 in the baked
        // mask (or trailing bits past the loaded canonical count) cannot set
        // a phantom active slot the GPU buffer doesn't carry.
        //
        // Union in the camera cell's fog mask: portal traversal can omit the
        // camera cell from `fog_reachable` in transient frames (e.g., crossing
        // a portal boundary), but fog the camera is inside must remain active
        // to prevent flicker. Idempotent when the camera cell is already in
        // `fog_reachable`.
        (false, Some(masks)) => {
            let mut active = union_active_mask(fog_reachable, masks);
            if let Some(cl) = camera_cell {
                active |= masks.get(cl as usize).copied().unwrap_or(0);
            }
            active & all_slots_mask
        }
        // Culled visibility + no mask table from the caller: fall back to
        // "all slots visible".
        // — `live_mask` will gate density-zero slots either way.
        // Note: when `canonical_volume_count == 0`, `all_slots_mask == 0` here,
        // so `active_count` will be 0 after repack and the fog pass is skipped
        // correctly via the `FogPass::active()` guard. No phantom slots are
        // activated on a zero-volume level.
        (false, None) => all_slots_mask,
    }
}

/// Returns `true` when `aabbs` is empty — conservative for pre-`set_fog_aabbs` frames;
/// spots are discarded by `FogPass::active()` before reaching the raymarch anyway.
pub(crate) fn sphere_intersects_any_fog_aabb(
    center: Vec3,
    radius: f32,
    aabbs: &[(Vec3, Vec3)],
) -> bool {
    if aabbs.is_empty() {
        return true;
    }
    let r2 = radius * radius;
    for (min, max) in aabbs {
        let clamped = center.clamp(*min, *max);
        let d = center - clamped;
        if d.length_squared() <= r2 {
            return true;
        }
    }
    false
}
