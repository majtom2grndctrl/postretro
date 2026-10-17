// Reflection-probe reference state: each animated light's value the capture bakes it at.
// See: context/lib/resource_management.md §5.2 · rendering_pipeline.md §4 (Animated lights)

use postretro_level_format::sh_volume::AnimationDescriptor;

/// Midpoint samples over one cycle for the reference mean. A power of two,
/// so every sample time `(i + ½) / N` is exact in f32 and the CPU parity test
/// evaluates the runtime reference at the very same times. Catmull-Rom
/// segments are cubics; with 4096 samples a curve of up to a few hundred keys
/// still gets several samples per segment, and the clamp's kinks cost at most
/// one sample's width each.
pub(crate) const REFERENCE_MEAN_SAMPLES: u32 = 4096;

/// The light's RGB value at the capture's reference state: the cycle mean of
/// the runtime's clamped evaluation (`sh_compose.wgsl` `animated_light_scale`,
/// mirrored on the CPU by `render_cpu::sh_compose::animated_light_scale`),
/// with the light treated as active. The mean is shift-invariant, so phase
/// and period drop out. A curveless descriptor (a script target given the
/// compiler's flat placeholder included) evaluates to its base colour at
/// every time, so its reference is its authored colour × intensity.
pub(crate) fn reference_value(descriptor: &AnimationDescriptor) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    for i in 0..REFERENCE_MEAN_SAMPLES {
        let cycle_t = (i as f32 + 0.5) / REFERENCE_MEAN_SAMPLES as f32;
        let value = clamped_scale(descriptor, cycle_t);
        for (acc, v) in sum.iter_mut().zip(value) {
            *acc += f64::from(v);
        }
    }
    sum.map(|s| (s / f64::from(REFERENCE_MEAN_SAMPLES)) as f32)
}

/// One evaluation at closed-loop cycle time `cycle_t` in `[0, 1)`: the
/// brightness spline clamped at 0, times the base colour, or the clamped
/// colour spline times the base colour when a colour curve is present.
pub(crate) fn clamped_scale(descriptor: &AnimationDescriptor, cycle_t: f32) -> [f32; 3] {
    let brightness = closed_catmull_rom(&descriptor.brightness, cycle_t, 1.0, |v| *v).max(0.0);
    let base = descriptor.base_color;
    let color = if descriptor.color.is_empty() {
        base
    } else {
        std::array::from_fn(|channel| {
            closed_catmull_rom(&descriptor.color, cycle_t, 1.0, |c| c[channel]).max(0.0)
                * base[channel]
        })
    };
    color.map(|c| c * brightness)
}

/// `curve_eval.wgsl`'s closed-loop Catmull-Rom (uniform knots, tension 0.5)
/// over one channel of `samples`; `empty` when there are none.
fn closed_catmull_rom<T>(
    samples: &[T],
    cycle_t: f32,
    empty: f32,
    channel: impl Fn(&T) -> f32,
) -> f32 {
    match samples {
        [] => empty,
        [value] => channel(value),
        _ => {
            let count = samples.len();
            let scaled = cycle_t * count as f32;
            let i1 = scaled.floor() as usize % count;
            let (i0, i2, i3) = ((i1 + count - 1) % count, (i1 + 1) % count, (i1 + 2) % count);
            let fraction = scaled.fract();
            let (p0, p1, p2, p3) = (
                channel(&samples[i0]),
                channel(&samples[i1]),
                channel(&samples[i2]),
                channel(&samples[i3]),
            );
            let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
            let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
            let c = -0.5 * p0 + 0.5 * p2;
            ((a * fraction + b) * fraction + c) * fraction + p1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{FalloffModel, LightAnimation, LightType, MapLight, ShadowType};
    use crate::sh_bake::animation_descriptor_for_light;
    use postretro_render_cpu::sh_compose::{AnimatedLightScaleDescriptor, animated_light_scale};

    fn descriptor(brightness: Vec<f32>, color: Vec<[f32; 3]>) -> AnimationDescriptor {
        AnimationDescriptor {
            period: 1.0,
            phase: 0.0,
            base_color: [2.0, 1.0, 0.5],
            brightness,
            color,
            direction: Vec::new(),
            start_active: 1,
        }
    }

    fn runtime(descriptor: &AnimationDescriptor, time: f32) -> [f32; 3] {
        animated_light_scale(
            Some(AnimatedLightScaleDescriptor {
                period: descriptor.period,
                phase: descriptor.phase,
                base_color: descriptor.base_color,
                brightness: &descriptor.brightness,
                color: &descriptor.color,
                is_active: true,
            }),
            time,
        )
    }

    /// The mean of the runtime reference at the mean's own sample times.
    fn runtime_mean(descriptor: &AnimationDescriptor) -> [f32; 3] {
        let mut sum = [0.0f64; 3];
        for i in 0..REFERENCE_MEAN_SAMPLES {
            let time = (i as f32 + 0.5) / REFERENCE_MEAN_SAMPLES as f32;
            for (acc, v) in sum.iter_mut().zip(runtime(descriptor, time)) {
                *acc += f64::from(v);
            }
        }
        sum.map(|s| (s / f64::from(REFERENCE_MEAN_SAMPLES)) as f32)
    }

    fn raw_brightness_mean(samples: &[f32]) -> f32 {
        let total: f64 = (0..REFERENCE_MEAN_SAMPLES)
            .map(|i| {
                let t = (i as f32 + 0.5) / REFERENCE_MEAN_SAMPLES as f32;
                f64::from(closed_catmull_rom(samples, t, 1.0, |v| *v))
            })
            .sum();
        (total / f64::from(REFERENCE_MEAN_SAMPLES)) as f32
    }

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= eps)
    }

    /// The compiler mirror is the runtime's clamped evaluation at every
    /// sample, curve shapes and both curve kinds included.
    #[test]
    fn clamped_scale_matches_the_runtime_compose_reference_at_every_sample() {
        let cases = [
            descriptor(Vec::new(), Vec::new()),
            descriptor(vec![0.7], Vec::new()),
            descriptor(vec![1.0, 0.0, 0.0, 1.0, 0.2], Vec::new()),
            descriptor(
                Vec::new(),
                vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ),
            descriptor(
                vec![0.0, 2.0, 0.0, 0.0, 1.5, 0.0],
                vec![
                    [1.0, 0.2, 0.0],
                    [0.0, 0.0, 0.0],
                    [0.3, 1.0, 0.9],
                    [1.0, 1.0, 1.0],
                ],
            ),
        ];
        for case in &cases {
            for i in 0..REFERENCE_MEAN_SAMPLES {
                let t = (i as f32 + 0.5) / REFERENCE_MEAN_SAMPLES as f32;
                let (ours, theirs) = (clamped_scale(case, t), runtime(case, t));
                assert!(
                    close(ours, theirs, 1e-6),
                    "{case:?} at {t}: {ours:?} vs {theirs:?}"
                );
            }
            assert!(
                close(reference_value(case), runtime_mean(case), 1e-6),
                "{case:?}"
            );
        }
    }

    // P18: a strobe whose spline overshoots below zero between keys.
    #[test]
    fn strobe_reference_is_the_mean_of_the_clamped_curve_not_the_raw_spline() {
        let strobe = vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let case = descriptor(strobe.clone(), Vec::new());
        let reference = reference_value(&case);
        let raw = raw_brightness_mean(&strobe);
        assert!(close(reference, runtime_mean(&case), 1e-6));
        // The overshoot dips below zero, so the clamp raises the mean.
        assert!(
            reference[1] > raw * case.base_color[1] + 1e-3,
            "clamped {reference:?} vs raw {raw}"
        );
    }

    // P18: colour and brightness curves both present: the mean of their
    // clamped product, not the product of their means.
    #[test]
    fn colour_and_brightness_reference_is_the_mean_of_their_clamped_product() {
        let case = descriptor(
            vec![1.0, 0.0, 0.0, 1.0],
            vec![
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
        );
        let reference = reference_value(&case);
        assert!(close(reference, runtime_mean(&case), 1e-6));
        let brightness = raw_brightness_mean(&case.brightness);
        let red = raw_brightness_mean(&case.color.iter().map(|c| c[0]).collect::<Vec<_>>());
        assert!(
            (reference[0] - brightness * red * case.base_color[0]).abs() > 1e-2,
            "{reference:?}"
        );
        assert_eq!(reference[1], 0.0);
    }

    /// A constant curve's mean is that constant: the sample count does not
    /// bias a flat light.
    #[test]
    fn a_flat_curve_references_its_value() {
        let case = descriptor(vec![0.5, 0.5, 0.5], Vec::new());
        assert!(close(reference_value(&case), [1.0, 0.5, 0.25], 1e-6));
    }

    fn light(animation: Option<LightAnimation>) -> MapLight {
        MapLight {
            origin: glam::DVec3::ZERO,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 3.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    // P9: a curveless light and a script target given the compiler's flat
    // placeholder reference their authored colour; a script target that keeps
    // its Quake `style` curve references that curve's mean. Either holds when
    // the light starts inactive.
    #[test]
    fn curveless_and_placeholder_lights_reference_their_authored_colour() {
        let authored = [3.0, 1.5, 0.75];
        for start_active in [true, false] {
            let placeholder = light(Some(crate::map_data::animated_light_placeholder(
                start_active,
            )));
            let reference = reference_value(&animation_descriptor_for_light(&placeholder));
            assert!(close(reference, authored, 1e-6), "{reference:?}");
        }
        let curveless = light(Some(LightAnimation {
            period: 2.5,
            phase: 0.3,
            brightness: None,
            color: None,
            direction: None,
            start_active: false,
        }));
        let reference = reference_value(&animation_descriptor_for_light(&curveless));
        assert!(close(reference, authored, 1e-6), "{reference:?}");
    }

    #[test]
    fn a_light_keeping_its_style_curve_references_that_curves_mean() {
        // Quake style 1 ("FLICKER (first variety)"), as the map translator
        // stores it: brightness samples over one period.
        let style: Vec<f32> = "mmnmmommommnonmmonqnmmo"
            .bytes()
            .map(|b| f32::from(b - b'a') / f32::from(b'm' - b'a'))
            .collect();
        for start_active in [true, false] {
            let styled = light(Some(LightAnimation {
                period: 2.3,
                phase: 0.0,
                brightness: Some(style.clone()),
                color: None,
                direction: None,
                start_active,
            }));
            let descriptor = animation_descriptor_for_light(&styled);
            let reference = reference_value(&descriptor);
            let mean = runtime_mean(&AnimationDescriptor {
                period: 1.0,
                phase: 0.0,
                ..descriptor.clone()
            });
            assert!(close(reference, mean, 1e-6), "{reference:?} vs {mean:?}");
            assert!(!close(reference, [3.0, 1.5, 0.75], 1e-3), "{reference:?}");
        }
    }
}
