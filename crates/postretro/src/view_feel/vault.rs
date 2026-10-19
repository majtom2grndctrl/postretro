// Camera pose held while vaulting, weighted by the vault's own progress.
// See: context/lib/movement.md §1

use postretro_foundation::VaultViewParams;
use super::ViewFeelState;
pub(crate) use super::pose::VaultViewInput;

/// Shortest ease back to neutral, seconds, after a vault ends with weight
/// left: an early end, or the end of a vault whose `easeOut` is 0. A longer
/// one comes from the climb's time × `easeOut`.
const MIN_RELEASE_S: f32 = 0.1;

/// Progress drop, as a share of the vault, that reads as a new vault rather
/// than a reconcile nudging the current one.
const RESTART_PROGRESS_DROP: f32 = 0.05;

/// Render-side integrator for the vault pose. Not tick state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct VaultViewState {
    /// Weight presented last frame, before the view-feel scale.
    weight: f32,
    /// Progress last frame, while vaulting.
    progress: Option<f32>,
    /// The weight a release started from, and how long it has run.
    release_from: f32,
    release_elapsed_s: f32,
    release_s: f32,
    /// Time since the pose started following this vault, seconds: the ease-in
    /// from neutral a vault that appears already in progress takes.
    join_elapsed_s: f32,
}

/// Held offsets this frame, in degrees, scaled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct VaultViewOutput {
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
    pub(crate) fov: f32,
}

/// Weight of the held pose at `progress`: up over the first `ease_in` share,
/// held, down over the last `ease_out` share to exactly 0 at the end. Both
/// ramps are smoothsteps, so the pose starts and settles with no velocity
/// step. A zero `ease_in` starts at full weight; a zero `ease_out` holds full
/// weight to the end, and the release that follows the vault's end
/// ([`MIN_RELEASE_S`]) eases it back.
fn envelope(params: &VaultViewParams, progress: f32) -> f32 {
    let up = if params.ease_in > 0.0 {
        smoothstep(progress / params.ease_in)
    } else {
        1.0
    };
    let down = if params.ease_out > 0.0 {
        smoothstep((1.0 - progress) / params.ease_out)
    } else {
        1.0
    };
    up.min(down)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Advance the vault pose by one frame.
///
/// While `vaulting` is set the weight is the envelope at the presented
/// progress, so it tracks the climb itself rather than a timer. Every other
/// way out — the envelope finishing, an abort, death, hot reload, a reconcile
/// that ends the vault with no edge — starts a release: the weight eases from
/// where it was to 0 on a smoothstep over the climb's time × `easeOut`, at
/// least [`MIN_RELEASE_S`]. A vault that ends with weight left (an `easeOut`
/// of 0) takes the same release.
///
/// A vault the pose was not following — one that starts, one a reconcile or
/// a mid-vault `clear_state_effects` shows already in progress, or a rewind
/// read as a new vault — eases in from neutral over the ease-in time (the
/// climb's time × `easeIn`) and is capped by that ramp until it meets the
/// envelope. The ramp counts this frame, so it never holds back a vault that
/// started this frame, which has run at most a frame. A release still
/// running meanwhile presents the larger of the two, so the pose continues
/// without a step. The weight advances under any scale, so restoring the
/// view-feel scale resumes from the live blend; `frame_dt = 0` holds
/// everything.
pub(crate) fn evaluate(
    params: Option<&VaultViewParams>,
    vaulting: Option<VaultViewInput>,
    state: &mut ViewFeelState,
    frame_dt: f32,
    scale: f32,
) -> VaultViewOutput {
    let Some(params) = params else {
        state.vault_view = VaultViewState::default();
        return VaultViewOutput::default();
    };
    let view = &mut state.vault_view;
    let rewound = match (vaulting, view.progress) {
        (Some(input), Some(last)) => input.progress < last - RESTART_PROGRESS_DROP,
        _ => false,
    };
    let joined = rewound || (vaulting.is_some() && view.progress.is_none());
    let ended = vaulting.is_none() && view.progress.is_some();
    if rewound || ended {
        view.release_from = view.weight;
        view.release_elapsed_s = 0.0;
    }
    if joined {
        view.join_elapsed_s = 0.0;
    }
    if let Some(input) = vaulting {
        view.release_s = (input.path_s * params.ease_out).max(MIN_RELEASE_S);
    }
    let frame_dt = frame_dt.max(0.0);
    view.release_elapsed_s += frame_dt;
    view.join_elapsed_s += frame_dt;
    let release = if view.release_s > 0.0 {
        view.release_from * (1.0 - smoothstep(view.release_elapsed_s / view.release_s))
    } else {
        0.0
    };
    view.weight = match vaulting {
        Some(input) => {
            let ease_in_s = input.path_s * params.ease_in;
            let join = if ease_in_s > 0.0 {
                smoothstep(view.join_elapsed_s / ease_in_s)
            } else {
                1.0
            };
            envelope(params, input.progress).min(join).max(release)
        }
        None => release,
    };
    view.progress = vaulting.map(|input| input.progress);

    let weight = view.weight * scale;
    VaultViewOutput {
        pitch: params.pitch * weight,
        roll: params.roll * weight,
        fov: params.fov * weight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_physics::movement::vault_path_ms;

    const DT: f32 = 1.0 / 60.0;
    const DURATION_S: f32 = 0.42;

    fn params() -> VaultViewParams {
        VaultViewParams {
            pitch: -6.0,
            roll: 2.0,
            fov: -3.0,
            ease_in: 0.15,
            ease_out: 0.2,
        }
    }

    fn vaulting(progress: f32) -> Option<VaultViewInput> {
        Some(VaultViewInput {
            progress,
            path_s: DURATION_S,
            landing: None,
        })
    }

    /// Present a whole vault at `frame_dt`, returning each frame's pitch.
    fn present_vault(state: &mut ViewFeelState, frame_dt: f32, until: f32) -> Vec<f32> {
        let frames = (until * DURATION_S / frame_dt).ceil() as usize;
        (0..=frames)
            .map(|frame| {
                let progress = (frame as f32 * frame_dt / DURATION_S).min(until);
                evaluate(Some(&params()), vaulting(progress), state, frame_dt, 1.0).pitch
            })
            .collect()
    }

    fn ease_out_frames(state: &mut ViewFeelState, frame_dt: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|_| evaluate(Some(&params()), None, state, frame_dt, 1.0).pitch)
            .collect()
    }

    #[test]
    fn the_pose_eases_in_holds_and_eases_out_with_the_vaults_progress() {
        let mut state = ViewFeelState::default();
        let pitches = present_vault(&mut state, DT, 1.0);
        assert_eq!(pitches[0], 0.0, "neutral as the vault starts");
        assert!(pitches.last().unwrap().abs() < 1.0e-6, "neutral as it ends");
        // The pose at `progress` of a vault followed from its start.
        let hold = |progress: f32| {
            let mut state = ViewFeelState::default();
            let frames = (progress * DURATION_S / DT).ceil() as usize;
            let mut output = VaultViewOutput::default();
            for frame in 0..=frames {
                let at = (frame as f32 * DT / DURATION_S).min(progress);
                output = evaluate(Some(&params()), vaulting(at), &mut state, DT, 1.0);
            }
            output
        };
        for progress in [0.15, 0.3, 0.5, 0.8] {
            let held = hold(progress);
            assert_eq!(
                held,
                VaultViewOutput {
                    pitch: -6.0,
                    roll: 2.0,
                    fov: -3.0
                },
                "{progress}"
            );
        }
        // Monotonic in, then out, with no step larger than a smooth ramp's.
        let peak = pitches.iter().position(|p| *p == -6.0).unwrap();
        for pair in pitches[..=peak].windows(2) {
            assert!(pair[1] <= pair[0] + 1.0e-6);
        }
        for pair in pitches[peak..].windows(2) {
            assert!(pair[1] >= pair[0] - 1.0e-6);
        }
        // Smooth ends: a linear ramp would be at 0.2° here, a smoothstep at 0.02°.
        for progress in [0.005, 0.995] {
            let edge = hold(progress).pitch;
            assert!(edge < 0.0 && edge > -0.05, "{progress}: {edge}");
        }
        // Ending normally leaves nothing to release.
        assert!(
            ease_out_frames(&mut state, DT, 30)
                .iter()
                .all(|p| p.abs() < 1.0e-6)
        );
    }

    #[test]
    fn an_early_end_eases_back_from_the_presented_weight_without_a_step() {
        // Held mid-vault, then a blocked path, death or a reconcile leaves it.
        let mut state = ViewFeelState::default();
        let pitches = present_vault(&mut state, DT, 0.5);
        let last = *pitches.last().unwrap();
        assert_eq!(last, -6.0);
        let release = ease_out_frames(&mut state, DT, 40);
        let steepest = release
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .fold(0.0_f32, f32::max);
        assert!(
            release[0] - last < 0.5 * steepest,
            "the release starts gently, with no step"
        );
        for pair in release.windows(2) {
            assert!(pair[1] >= pair[0] - 1.0e-6, "monotonic back to neutral");
        }
        let release_s = (DURATION_S * params().ease_out).max(MIN_RELEASE_S);
        let release_frames = (release_s / DT).ceil() as usize;
        assert!(
            release[release_frames].abs() < 1.0e-6,
            "neutral after the release"
        );
        assert!(release[release_frames / 2] < -1.0, "a release, not a snap");

        // Mid ease-in: the release starts where the pose had got to.
        let mut state = ViewFeelState::default();
        let rising = present_vault(&mut state, DT, 0.06);
        let release = ease_out_frames(&mut state, DT, 20);
        assert!(release[0] <= 0.0 && release[0] >= *rising.last().unwrap() - 1.0e-6);
        assert!(release[19].abs() < 1.0e-6);
    }

    #[test]
    fn a_vault_started_during_a_release_continues_from_it() {
        let mut state = ViewFeelState::default();
        present_vault(&mut state, DT, 0.5);
        let release = ease_out_frames(&mut state, DT, 2);
        let mut previous = *release.last().unwrap();
        assert!(previous < -4.0, "most of the pose is still presented");
        // No frame moves further than the dev ease-in's steepest frame (6° over
        // 63 ms on a smoothstep: 2.4° at 60 Hz).
        let smooth = 2.5;
        // A new vault starts at progress 0 while the release still holds most
        // of the pose; then a reconcile rewinds the next vault to its start.
        for frame in 0..20 {
            let progress = frame as f32 * DT / DURATION_S;
            let pitch = evaluate(Some(&params()), vaulting(progress), &mut state, DT, 1.0).pitch;
            assert!(
                (pitch - previous).abs() < smooth,
                "frame {frame}: {previous} → {pitch}"
            );
            previous = pitch;
        }
        assert_eq!(previous, -6.0, "the new vault holds its pose");
        let rewound = evaluate(Some(&params()), vaulting(0.0), &mut state, DT, 1.0).pitch;
        assert!(
            (rewound - previous).abs() < 1.0,
            "a rewind eases, {previous} → {rewound}"
        );
    }

    #[test]
    fn the_pose_is_frame_rate_independent_and_zero_dt_holds() {
        for (slow_dt, fast_dt) in [(1.0 / 30.0, 1.0 / 240.0), (1.0 / 60.0, 1.0 / 240.0)] {
            let mut slow = ViewFeelState::default();
            let mut fast = ViewFeelState::default();
            present_vault(&mut slow, slow_dt, 0.5);
            present_vault(&mut fast, fast_dt, 0.5);
            let a = ease_out_frames(&mut slow, slow_dt, 2);
            let b = ease_out_frames(
                &mut fast,
                fast_dt,
                (2.0 * slow_dt / fast_dt).round() as usize,
            );
            assert!(
                (a[1] - b[b.len() - 1]).abs() < 1.0e-3,
                "{a:?} vs {}",
                b[b.len() - 1]
            );
        }
        let mut state = ViewFeelState::default();
        present_vault(&mut state, DT, 0.5);
        ease_out_frames(&mut state, DT, 3);
        let frozen = state;
        let held = evaluate(Some(&params()), None, &mut state, 0.0, 1.0);
        let again = evaluate(Some(&params()), None, &mut state, 0.0, 1.0);
        assert_eq!(held, again);
        assert_eq!(state.vault_view.weight, frozen.vault_view.weight);
    }

    #[test]
    fn the_view_feel_scale_scales_the_pose_without_freezing_it_and_clearing_resets_it() {
        let mut state = ViewFeelState::default();
        // Longer than the ease-in, all at zero scale.
        for _ in 0..10 {
            let reduced = evaluate(Some(&params()), vaulting(0.5), &mut state, DT, 0.0);
            assert_eq!(
                reduced,
                VaultViewOutput::default(),
                "reduced motion shows no pose"
            );
        }
        let half = evaluate(Some(&params()), vaulting(0.5), &mut state, DT, 0.5);
        assert_eq!(half.pitch, -3.0);
        // The blend kept advancing at zero scale: restoring it resumes live.
        let full = evaluate(Some(&params()), vaulting(0.5), &mut state, DT, 1.0);
        assert_eq!(full.pitch, -6.0);
        state.clear_state_effects();
        assert_eq!(
            evaluate(Some(&params()), None, &mut state, DT, 1.0),
            VaultViewOutput::default()
        );
        // No block: no pose, and no state left to release later.
        evaluate(Some(&params()), vaulting(0.5), &mut state, DT, 1.0);
        assert_eq!(
            evaluate(None, vaulting(0.5), &mut state, DT, 1.0),
            VaultViewOutput::default()
        );
        assert_eq!(
            evaluate(Some(&params()), None, &mut state, DT, 1.0),
            VaultViewOutput::default()
        );
    }

    #[test]
    fn presented_progress_trails_the_latest_tick_by_the_unpresented_share() {
        let tick_s = 1.0 / 60.0;
        let step_ms = tick_s * 1000.0;
        // 420 ms runs 26 whole ticks at 60 Hz.
        let path_ms = vault_path_ms(420.0, step_ms);
        assert_eq!(
            path_ms,
            postretro_physics::movement::vault_clock(26, step_ms)
        );
        // The entry tick leaves one step elapsed; at alpha 0 the camera still
        // shows the pose before it.
        assert_eq!(
            VaultViewInput::presented(step_ms, 420.0, tick_s, 0.0, None).progress,
            0.0
        );
        let half = VaultViewInput::presented(step_ms, 420.0, tick_s, 0.5, None).progress;
        assert!((half - 0.5 * step_ms / path_ms).abs() < 1.0e-6);
        assert_eq!(
            VaultViewInput::presented(path_ms + step_ms, 420.0, tick_s, 1.0, None).progress,
            1.0
        );
        assert_eq!(
            VaultViewInput::presented(path_ms, 420.0, tick_s, 1.0, None).path_s,
            path_ms / 1000.0
        );
    }

    // Regression: progress divided by `durationMs` while the path ran whole
    // ticks past it, so the pose reached neutral before the pawn reached the top.
    #[test]
    fn the_pose_reaches_neutral_on_the_tick_the_path_reaches_the_top() {
        use postretro_physics::movement::{vault_clock, vault_path_ticks};
        for hz in [30.0_f32, 60.0, 144.0, 240.0] {
            let tick_s = 1.0 / hz;
            let step_ms = tick_s * 1000.0;
            let ticks = vault_path_ticks(420.0, step_ms);
            let at_top = VaultViewInput::presented(vault_clock(ticks, step_ms), 420.0, tick_s, 1.0, None);
            assert_eq!(
                at_top.progress, 1.0,
                "{hz} Hz: neutral as the top is reached"
            );
            assert_eq!(envelope(&params(), at_top.progress), 0.0);
            let before =
                VaultViewInput::presented(vault_clock(ticks - 1, step_ms), 420.0, tick_s, 1.0, None);
            assert!(
                envelope(&params(), before.progress) > 0.0,
                "{hz} Hz: still easing out a tick before the top"
            );
        }
    }

    // Regression: a vault the pose had not followed snapped straight to its
    // envelope.
    #[test]
    fn a_vault_that_appears_mid_climb_eases_in_from_neutral() {
        // A reconcile starts a vault the client never predicted, half way up;
        // or a descriptor change clears view-feel state mid-vault.
        let mut cleared = ViewFeelState::default();
        present_vault(&mut cleared, DT, 0.5);
        cleared.clear_state_effects();
        for mut state in [ViewFeelState::default(), cleared] {
            let mut previous = 0.0_f32;
            // Seven frames stay inside the hold, short of the ease-out at 0.8.
            let pitches: Vec<f32> = (0..7)
                .map(|frame| {
                    let progress = 0.5 + frame as f32 * DT / DURATION_S;
                    evaluate(Some(&params()), vaulting(progress), &mut state, DT, 1.0).pitch
                })
                .collect();
            assert!(pitches[0] > -1.5, "no snap to the held pose: {pitches:?}");
            for pitch in &pitches {
                assert!(
                    *pitch <= previous + 1.0e-6,
                    "eases in monotonically: {pitches:?}"
                );
                assert!((pitch - previous).abs() < 2.5, "no step: {pitches:?}");
                previous = *pitch;
            }
            // In after the ease-in time (0.15 × 0.42 s, four frames), then the
            // envelope's hold.
            let ease_in_frames = (params().ease_in * DURATION_S / DT).ceil() as usize;
            assert_eq!(pitches[ease_in_frames], -6.0, "{pitches:?}");
            assert!(
                pitches[ease_in_frames / 2] < -1.0,
                "an ease, not a hold at neutral"
            );
        }
    }

    #[test]
    fn a_zero_ease_out_holds_the_pose_to_the_end_then_releases_it_over_the_minimum_release() {
        let held_to_end = VaultViewParams {
            ease_out: 0.0,
            ..params()
        };
        let mut state = ViewFeelState::default();
        let frames = (DURATION_S / DT).ceil() as usize;
        let mut last = 0.0;
        for frame in 0..=frames {
            let progress = (frame as f32 * DT / DURATION_S).min(1.0);
            last = evaluate(Some(&held_to_end), vaulting(progress), &mut state, DT, 1.0).pitch;
        }
        assert_eq!(last, -6.0, "full weight as the climb ends");
        let release: Vec<f32> = (0..10)
            .map(|_| evaluate(Some(&held_to_end), None, &mut state, DT, 1.0).pitch)
            .collect();
        assert!(release[0] < -5.0, "no drop at the end: {release:?}");
        let release_frames = (MIN_RELEASE_S / DT).ceil() as usize;
        assert!(release[release_frames - 1].abs() < 1.0e-6, "{release:?}");
        assert!(release[release_frames / 2] < -1.0, "a release, not a snap");
    }
}
