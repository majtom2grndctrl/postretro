// Tests: the reflection tier's fractions and target extents, the pure half of
// the tier-extent rule.
// See: context/lib/testing_guide.md

use super::*;

// High is the default; ALL runs lowest first; Off alone does not reflect.
#[test]
fn fluid_reflection_quality_defaults_to_high_and_off_alone_does_not_reflect() {
    assert_eq!(
        FluidReflectionQuality::default(),
        FluidReflectionQuality::High
    );
    assert_eq!(
        FluidReflectionQuality::ALL,
        [
            FluidReflectionQuality::Off,
            FluidReflectionQuality::Low,
            FluidReflectionQuality::Medium,
            FluidReflectionQuality::High,
        ]
    );
    for quality in FluidReflectionQuality::ALL {
        assert_eq!(quality.reflects(), quality != FluidReflectionQuality::Off);
    }
}

// The fractions per axis: none at Off, then a quarter, a half and full.
#[test]
fn fluid_reflection_quality_fractions_are_quarter_half_and_full() {
    let fractions: Vec<_> = FluidReflectionQuality::ALL
        .iter()
        .map(|quality| quality.target_fraction())
        .collect();
    assert_eq!(fractions, [None, Some(0.25), Some(0.5), Some(1.0)]);
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
// 1×1; Off has no target at any extent.
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
        for (quality, divisor) in [
            (FluidReflectionQuality::Low, 4),
            (FluidReflectionQuality::Medium, 2),
            (FluidReflectionQuality::High, 1),
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
        FluidReflectionQuality::Low.target_extent((3840, 2160)),
        Some((960, 540))
    );
    assert_eq!(
        FluidReflectionQuality::Medium.target_extent((1921, 1081)),
        Some((961, 541))
    );
}
