// Connected-client mover correction glide: how fast a smoothed correction
// converges, advanced once per frame.
// See: context/lib/networking.md §Snapshot apply ordering

use postretro_entities::EntityRegistry;

/// Time constant of a mover correction's glide, in seconds. Faster than the
/// pawn offset's glide (0.7 per 60 Hz tick, about 47 ms), so a deck settles
/// under a rider before the rider's own residual does.
pub(crate) const MOVER_GLIDE_TAU_SECS: f32 = 0.025;

/// A glide within this many metres and radians is done.
pub(crate) const MOVER_GLIDE_SETTLED: f32 = 1.0e-5;

/// Advance every mover correction glide by `frame_dt` seconds of presented
/// time, at a rate independent of frame rate. Call once per frame, after the
/// fixed ticks and before any presentation consumer reads a mover's pose. A
/// glide seeded this frame is drawn whole first. No-op without glides.
pub fn advance_mover_correction_glides(registry: &mut EntityRegistry, frame_dt: f32) {
    if !registry.has_presentation_corrections() {
        return;
    }
    let remaining = if frame_dt.is_finite() && frame_dt > 0.0 {
        (-frame_dt / MOVER_GLIDE_TAU_SECS).exp()
    } else {
        1.0
    };
    registry.advance_presentation_corrections(remaining, MOVER_GLIDE_SETTLED);
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec3};
    use postretro_entities::{EntityRegistry, Transform};

    use super::advance_mover_correction_glides;

    fn glide_after(frame_hz: f32, elapsed_secs: f32) -> f32 {
        let mut registry = EntityRegistry::new();
        let mover = registry.spawn(Transform::default());
        registry
            .correct_transform(
                mover,
                Transform {
                    position: Vec3::new(0.16, 0.0, 0.0),
                    rotation: Quat::from_rotation_y(0.1),
                    ..Transform::default()
                },
                true,
            )
            .unwrap();
        let dt = 1.0 / frame_hz;
        // The correction frame draws it whole; each later frame shrinks it.
        advance_mover_correction_glides(&mut registry, dt);
        let frames = (elapsed_secs * frame_hz).round() as u32;
        for _ in 0..frames {
            advance_mover_correction_glides(&mut registry, dt);
        }
        registry
            .presentation_correction(mover)
            .map_or(0.0, |correction| correction.translation.length())
    }

    #[test]
    fn mover_glide_converges_in_the_same_presented_time_at_any_frame_rate() {
        for elapsed in [0.1, 0.2] {
            let at_60 = glide_after(60.0, elapsed);
            assert!(at_60 > 0.0);
            for hz in [30.0, 240.0] {
                let other = glide_after(hz, elapsed);
                assert!(
                    (other - at_60).abs() <= at_60 * 1.0e-4,
                    "{hz} Hz after {elapsed} s: {other} vs 60 Hz {at_60}"
                );
            }
        }
        // Converged and dropped well inside half a second.
        assert_eq!(glide_after(60.0, 0.5), 0.0);
    }
}
