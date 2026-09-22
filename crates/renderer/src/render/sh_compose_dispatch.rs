// Shared SH compose dispatch decisions.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_render_cpu::frame_uniforms::LightTermMask;

/// The compose atlas must be refreshed for new animated input, its initial
/// base copy, the frame after animated input stops, or a changed light-term
/// mask. Keeping that decision independent of individual compose pipelines is
/// the seam where streamed dirty ranges will replace whole-grid dispatches.
pub(super) fn should_dispatch(
    active: bool,
    pending_copy_through: bool,
    was_active: bool,
    frame_light_term_mask: LightTermMask,
    last_composed_mask: LightTermMask,
) -> bool {
    active || pending_copy_through || was_active || frame_light_term_mask != last_composed_mask
}

/// Empty affinity dimensions still need one copy-through workgroup so the
/// dummy atlas has a defined initial value.
pub(super) fn whole_grid_workgroups(dimensions: [u32; 3]) -> [u32; 3] {
    [
        dimensions[0].max(1),
        dimensions[1].max(1),
        dimensions[2].max(1),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_for_initial_copy_activity_transition_or_mask_change() {
        assert!(should_dispatch(
            false,
            true,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(should_dispatch(
            false,
            false,
            true,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(should_dispatch(
            false,
            false,
            false,
            LightTermMask::AMBIENT_FLOOR,
            LightTermMask::ALL,
        ));
        assert!(!should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn empty_dimensions_keep_the_dummy_copy_through_dispatch_valid() {
        assert_eq!(whole_grid_workgroups([0, 0, 0]), [1, 1, 1]);
        assert_eq!(whole_grid_workgroups([2, 3, 4]), [2, 3, 4]);
    }
}
