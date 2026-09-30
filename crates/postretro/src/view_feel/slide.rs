// Sustained slide camera presentation, independent of tick-side movement.
// See: context/lib/movement.md

use postretro_foundation::{MovementStateKind, SlideViewParams};

use super::ViewFeelState;

/// Additional slide dip must leave this much clearance above the pawn's feet.
pub(crate) const MIN_EYE_CLEARANCE: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct SlideViewOutput {
    pub(crate) eye_drop: f32,
    pub(crate) fov_increase: f32,
}

pub(crate) fn evaluate(
    params: Option<&SlideViewParams>,
    movement_state: MovementStateKind,
    state: &mut ViewFeelState,
    frame_dt: f32,
    scale: f32,
) -> SlideViewOutput {
    let Some(params) = params else {
        state.slide_blend = 0.0;
        return SlideViewOutput::default();
    };
    let sliding = movement_state == MovementStateKind::Slide;
    let target = if sliding { 1.0 } else { 0.0 };
    let rate = if sliding {
        params.enter_rate
    } else {
        params.exit_rate
    };
    let alpha = 1.0 - (-rate * frame_dt.max(0.0)).exp();
    state.slide_blend += (target - state.slide_blend) * alpha;
    // Advance even under reduced motion so restoring it uses the live state.
    let blend = state.slide_blend * scale;
    SlideViewOutput {
        eye_drop: params.eye_drop * blend,
        fov_increase: params.fov_increase * blend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> SlideViewParams {
        SlideViewParams {
            eye_drop: 0.15,
            fov_increase: 5.0,
            enter_rate: 18.0,
            exit_rate: 12.0,
        }
    }

    fn held(
        state: &mut ViewFeelState,
        kind: MovementStateKind,
        dt: f32,
        count: usize,
    ) -> SlideViewOutput {
        let mut output = SlideViewOutput::default();
        for _ in 0..count {
            output = evaluate(Some(&params()), kind, state, dt, 1.0);
        }
        output
    }

    #[test]
    fn slide_view_holds_until_every_non_slide_state_eases_it_out() {
        for kind in [
            MovementStateKind::Normal,
            MovementStateKind::Crouch,
            MovementStateKind::Dash,
        ] {
            let mut state = ViewFeelState::default();
            let active = held(&mut state, MovementStateKind::Slide, 1.0 / 60.0, 180);
            assert!((active.eye_drop - 0.15).abs() < 1e-5);
            assert!((active.fov_increase - 5.0).abs() < 1e-5);
            let exiting = held(&mut state, kind, 1.0 / 60.0, 1);
            assert!(exiting.eye_drop > 0.0 && exiting.eye_drop < active.eye_drop);
            assert!(exiting.fov_increase > 0.0 && exiting.fov_increase < active.fov_increase);
            let neutral = held(&mut state, kind, 1.0 / 60.0, 120);
            assert!(neutral.eye_drop < 1e-5 && neutral.fov_increase < 1e-5);
        }
    }

    #[test]
    fn slide_view_is_frame_rate_independent_and_zero_dt_holds() {
        let mut slow = ViewFeelState::default();
        let mut fast = ViewFeelState::default();
        let a = held(&mut slow, MovementStateKind::Slide, 1.0 / 30.0, 15);
        let b = held(&mut fast, MovementStateKind::Slide, 1.0 / 240.0, 120);
        assert!((a.eye_drop - b.eye_drop).abs() < 1e-6);
        assert!((a.fov_increase - b.fov_increase).abs() < 1e-5);
        let frozen = slow;
        let zero = evaluate(
            Some(&params()),
            MovementStateKind::Crouch,
            &mut slow,
            0.0,
            1.0,
        );
        assert_eq!(a, zero);
        assert_eq!(slow, frozen);
        let a = held(&mut slow, MovementStateKind::Crouch, 1.0 / 30.0, 15);
        let b = held(&mut fast, MovementStateKind::Crouch, 1.0 / 240.0, 120);
        assert!((a.eye_drop - b.eye_drop).abs() < 1e-6);
        assert!((a.fov_increase - b.fov_increase).abs() < 1e-5);
    }

    #[test]
    fn slide_view_motion_scale_suppresses_output_without_freezing_and_reset_clears_it() {
        let mut state = ViewFeelState::default();
        for _ in 0..120 {
            assert_eq!(
                evaluate(
                    Some(&params()),
                    MovementStateKind::Slide,
                    &mut state,
                    1.0 / 60.0,
                    0.0
                ),
                SlideViewOutput::default()
            );
        }
        let full = evaluate(
            Some(&params()),
            MovementStateKind::Slide,
            &mut state,
            0.0,
            1.0,
        );
        let half = evaluate(
            Some(&params()),
            MovementStateKind::Slide,
            &mut state,
            0.0,
            0.5,
        );
        assert!((full.eye_drop - 0.15).abs() < 1e-5);
        assert_eq!(half.eye_drop, full.eye_drop * 0.5);
        assert_eq!(half.fov_increase, full.fov_increase * 0.5);
        state.clear_state_effects();
        assert_eq!(
            evaluate(
                Some(&params()),
                MovementStateKind::Slide,
                &mut state,
                0.0,
                1.0
            ),
            SlideViewOutput::default()
        );
        held(&mut state, MovementStateKind::Slide, 1.0 / 60.0, 60);
        assert_eq!(
            evaluate(None, MovementStateKind::Slide, &mut state, 1.0 / 60.0, 1.0),
            SlideViewOutput::default()
        );
        assert_eq!(
            evaluate(
                Some(&params()),
                MovementStateKind::Normal,
                &mut state,
                0.0,
                1.0
            ),
            SlideViewOutput::default()
        );
    }
}
