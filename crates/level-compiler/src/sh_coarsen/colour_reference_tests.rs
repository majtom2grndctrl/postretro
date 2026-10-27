// Coloured indirect changes the existing id-41 quality reference, never its direct samples.
// See: context/plans/in-progress/coloured-sh-bounce/index.md §Acceptance

use super::*;
use crate::delta_drop_policy::ScriptMutableDescriptorSlots;
use crate::delta_sections::PostBakeDeltaSections;
use crate::sh_runtime_envelope::apply_runtime_safe_envelope;
use postretro_level_format::sh_volume::OctahedralShProbe;

#[test]
fn changed_indirect_reference_keeps_direct_samples_quality_protection_and_seams() {
    // Existing dense-reference irradiance units; this does not assert rendered
    // pixel error at the renderer's indirect scale or arbitrary diagnostic scales.
    let dims = [12, 4, 4];
    let validity = all_valid(dims);
    let mut old_indirect = bright_base_indirect(dims, 100.0);
    old_indirect.probes = vec![
        OctahedralShProbe {
            validity: 1,
            ..Default::default()
        };
        validity.len()
    ];
    let mut new_indirect = bright_base_indirect(dims, 0.25);
    new_indirect.probes = old_indirect.probes.clone();
    // A dim red composed reference; preserve the same direct tiles below.
    for texel in new_indirect.compact_atlas.chunks_exact_mut(8) {
        for (channel, value) in [0.25, 0.05, 0.05].into_iter().enumerate() {
            texel[channel * 2..channel * 2 + 2]
                .copy_from_slice(&f32_to_f16_bits(value).to_le_bytes());
        }
    }
    let mut direct = direct_delta_row(&[1.0, 1.0, 1.0]);
    for local in 0..PROBES_PER_CELL {
        let value = f32_to_f16_bits(1.0 + parity(local, 1.0));
        let at = (PROBES_PER_CELL + local) * DELTA_TILE_TEXEL_F16_COUNT;
        direct.delta_subblocks[at..at + 3].fill(value);
    }
    let direct_samples = direct.delta_subblocks.clone();
    let params = CoarsenParams::default();
    let protect = [[0.1, 0.1, 0.1, 0.2, 0.2, 0.2]];
    let classify = |base, protect: &[[f32; 6]]| {
        classify_direct_levels(
            base,
            None,
            DeltaSectionsRef {
                direct: Some(&direct),
                ..Default::default()
            },
            grid_ref(dims, &validity),
            protect,
            &params,
        )
    };
    assert_eq!(classify(&old_indirect, &[]), [L2, L2, L2]);
    assert_eq!(
        classify(&new_indirect, &[]),
        [L1, L0, L1],
        "same direct residual is too large relative to dimmer indirect; neighboring seams refine"
    );
    let old_levels = classify(&old_indirect, &protect);
    let new_levels = classify(&new_indirect, &protect);
    assert_eq!(old_levels, [L0, L1, L2]);
    assert_eq!(new_levels, [L0, L0, L1]);

    for (base, levels) in [
        (&old_indirect, old_levels.clone()),
        (&new_indirect, new_levels.clone()),
    ] {
        let mut candidate = direct.clone();
        candidate.cell_levels = levels.clone();
        let mut sections =
            PostBakeDeltaSections::new(Default::default(), None, None, Some(candidate), None);
        let report = apply_runtime_safe_envelope(
            base,
            None,
            &mut sections,
            &ScriptMutableDescriptorSlots::empty(0),
            &params,
        )
        .unwrap();
        assert_eq!(
            report.failures_after_repair, 0,
            "exact emitted f16 envelope retains existing quality bounds"
        );
        let emitted_source = sections.direct.unwrap();
        assert_eq!(
            emitted_source.delta_subblocks, direct_samples,
            "classification and repair cannot repaint direct samples"
        );
        assert_eq!(emitted_source.cell_levels, levels);
        assert_eq!(
            emitted_source.cell_levels[0], L0,
            "protection survives envelope repair"
        );
        for pair in emitted_source.cell_levels.windows(2) {
            assert!(
                pair[0].abs_diff(pair[1]) <= 1,
                "participating seams retain the one-level bound"
            );
        }
        eprintln!(
            "[id41 coloured reference] map_p95={}, floor={}, levels={:?}, repaired={}",
            report.dense_reference_map_p95,
            report.darkness_floor,
            emitted_source.cell_levels,
            report.selected_l0_restores
        );
    }

    // Reusing the old bright-reference choices on the new dim reference is
    // insufficient: the runtime-weight envelope must detect and repair it.
    let mut stale_candidate = direct.clone();
    stale_candidate.cell_levels = old_levels;
    let mut stale =
        PostBakeDeltaSections::new(Default::default(), None, None, Some(stale_candidate), None);
    let report = apply_runtime_safe_envelope(
        &new_indirect,
        None,
        &mut stale,
        &ScriptMutableDescriptorSlots::empty(0),
        &params,
    )
    .unwrap();
    assert!(report.failures_before_repair > 0 && report.selected_l0_restores > 0);
    assert_eq!(report.failures_after_repair, 0);
    let repaired = stale.direct.unwrap();
    assert_eq!(repaired.cell_levels, new_levels);
    assert_eq!(repaired.delta_subblocks, direct_samples);
}
