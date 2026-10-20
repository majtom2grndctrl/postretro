// Tests: the reflection tier's presets, fractions and target extents, the
// pure half of the tier-extent rule.
// See: context/lib/testing_guide.md

use super::*;

// High is the default; ALL runs lowest first. Off alone draws no probe term;
// Low draws it without the screen-space layer, which medium and above alone
// run.
#[test]
fn fluid_reflection_quality_defaults_to_high_and_low_draws_the_probe_term_alone() {
    use FluidReflectionQuality::*;
    assert_eq!(FluidReflectionQuality::default(), High);
    assert_eq!(
        FluidReflectionQuality::ALL,
        [Off, Low, Medium, High, VeryHigh, Ultra]
    );
    let predicates: Vec<_> = FluidReflectionQuality::ALL
        .iter()
        .map(|quality| (quality.draws_probe_term(), quality.runs_screen_space()))
        .collect();
    assert_eq!(
        predicates,
        [
            (false, false),
            (true, false),
            (true, true),
            (true, true),
            (true, true),
            (true, true),
        ]
    );
}

// Every screen-space tier's preset, pinned from the table: fraction, steps,
// stride divisor and thickness. Off and Low have none.
#[test]
fn every_fluid_reflection_tier_pins_its_preset() {
    use FluidReflectionQuality::*;
    let presets: Vec<_> = FluidReflectionQuality::ALL
        .iter()
        .map(|quality| {
            quality.preset().map(|preset| {
                (
                    preset.fraction,
                    preset.march.step_budget,
                    preset.march.rows_per_stride_pixel,
                    preset.march.thickness,
                )
            })
        })
        .collect();
    assert_eq!(
        presets,
        [
            None,
            None,
            Some((0.5, 48, 180, 1.0)),
            Some((1.0, 96, 270, 1.0)),
            Some((1.0, 192, 180, 1.5)),
            Some((1.0, 256, 135, 2.5)),
        ]
    );
    // Each preset is already sanitized: the dev tuning's rules change none.
    for quality in [Medium, High, VeryHigh, Ultra] {
        let march = quality.preset().unwrap().march;
        assert_eq!(march.sanitized(march), march, "{quality:?}");
    }
    // The default march is the default tier's.
    assert_eq!(
        FluidReflectionMarch::default(),
        High.preset().unwrap().march
    );
}

// The dev tuning's rules: at least one step and one row per stride pixel, a
// finite non-negative thickness, and a non-finite thickness keeps the
// current one.
#[test]
fn fluid_reflection_march_sanitizes_by_the_dev_tuning_rules() {
    let current = FluidReflectionMarch::default();
    let degenerate = FluidReflectionMarch {
        step_budget: 0,
        rows_per_stride_pixel: 0,
        thickness: -1.0,
    };
    assert_eq!(
        degenerate.sanitized(current),
        FluidReflectionMarch {
            step_budget: 1,
            rows_per_stride_pixel: 1,
            thickness: 0.0,
        }
    );
    let nan = FluidReflectionMarch {
        thickness: f32::NAN,
        ..current
    };
    assert_eq!(nan.sanitized(current).thickness, current.thickness);
    // Stride: one full-resolution pixel per divisor's rows, at least one.
    let march = FluidReflectionMarch {
        rows_per_stride_pixel: 270,
        ..current
    };
    assert_eq!(march.stride((3840, 2160)), 8.0);
    assert_eq!(march.stride((1920, 1080)), 4.0);
    assert_eq!(march.stride((640, 360)), 1.0);
    assert_eq!(march.stride((64, 48)), 1.0);
}

// The fractions per axis: none at Off and Low, then half, then full.
#[test]
fn fluid_reflection_quality_fractions_follow_the_presets() {
    let fractions: Vec<_> = FluidReflectionQuality::ALL
        .iter()
        .map(|quality| quality.target_fraction())
        .collect();
    assert_eq!(
        fractions,
        [None, None, Some(0.5), Some(1.0), Some(1.0), Some(1.0)]
    );
}

// The target extent per axis at a fraction of the scene's: rounded up, never
// below one texel.
#[test]
fn fluid_reflection_target_extent_rounds_up_to_at_least_one_texel() {
    assert_eq!(reflection_target_extent((1920, 1080), 1.0), (1920, 1080));
    assert_eq!(reflection_target_extent((1921, 1081), 0.5), (961, 541));
    assert_eq!(reflection_target_extent((3, 2), 0.25), (1, 1));
    assert_eq!(reflection_target_extent((1, 1), 0.25), (1, 1));
}

// Each tier resolves at its fraction of the scene extent, ceil and at least
// 1×1; Off and Low have no target at any extent.
#[test]
fn each_fluid_reflection_tier_resolves_at_its_fraction_of_the_scene() {
    let scenes = [
        (3840, 2160),
        (1921, 1081),
        (640, 360),
        (7, 5),
        (3, 2),
        (1, 1),
    ];
    for scene in scenes {
        assert_eq!(FluidReflectionQuality::Off.target_extent(scene), None);
        assert_eq!(FluidReflectionQuality::Low.target_extent(scene), None);
        for (quality, divisor) in [
            (FluidReflectionQuality::Medium, 2),
            (FluidReflectionQuality::High, 1),
            (FluidReflectionQuality::VeryHigh, 1),
            (FluidReflectionQuality::Ultra, 1),
        ] {
            let expected = (
                scene.0.div_ceil(divisor).max(1),
                scene.1.div_ceil(divisor).max(1),
            );
            assert_eq!(
                quality.target_extent(scene),
                Some(expected),
                "{quality:?} at {scene:?}"
            );
        }
    }
    assert_eq!(
        FluidReflectionQuality::Medium.target_extent((1921, 1081)),
        Some((961, 541))
    );
}
