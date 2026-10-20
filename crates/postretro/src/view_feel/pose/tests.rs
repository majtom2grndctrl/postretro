// Tests: `viewFeel.pose` rows, their timing, overlap and ledge focus.

use glam::Vec3;
use postretro_foundation::{
    LedgeFocus, MovementStateKind, PoseChannels, PoseParams, PoseRow, VaultPoseRow,
};

use super::vault_row::{self, LEDGE_FOCUS_MIN_HORIZONTAL_M, MIN_RELEASE_S};
use super::{PoseFrame, PoseOutput, PoseState, VaultViewInput, evaluate};

const DT: f32 = 1.0 / 60.0;
const PATH_S: f32 = 0.42;
/// A still eye 1.5 m above the floor; the default landing spot is 0.5 m below
/// it and 0.6 m ahead, so its line is atan2(-0.5, 0.6) ≈ -39.8°.
const EYE: Vec3 = Vec3::new(0.0, 1.5, 0.0);
const LANDING: Vec3 = Vec3::new(0.6, 1.0, 0.0);
const EPS: f32 = 1.0e-4;

fn line_deg(eye: Vec3, landing: Vec3) -> f32 {
    let horizontal = glam::Vec2::new(landing.x - eye.x, landing.z - eye.z)
        .length()
        .max(LEDGE_FOCUS_MIN_HORIZONTAL_M);
    (landing.y - eye.y).atan2(horizontal).to_degrees()
}

fn channels(pitch: f32, roll: f32, eye_drop: f32, fov: f32) -> PoseChannels {
    PoseChannels {
        pitch,
        roll,
        eye_drop,
        fov,
    }
}

fn slide_row() -> PoseRow {
    PoseRow {
        channels: channels(-2.0, 1.0, 0.15, 5.0),
        enter_rate: 18.0,
        exit_rate: 12.0,
    }
}

fn vault_row(pitch: f32, focus: Option<LedgeFocus>) -> VaultPoseRow {
    VaultPoseRow {
        channels: channels(pitch, 2.0, 0.0, -3.0),
        ease_in: 0.15,
        ease_out: 0.2,
        ledge_focus: focus,
    }
}

fn focus(strength: f32, max_pitch: f32) -> Option<LedgeFocus> {
    Some(LedgeFocus {
        strength,
        max_pitch,
    })
}

fn only_slide() -> PoseParams {
    PoseParams {
        slide: Some(slide_row()),
        vault: None,
    }
}

fn only_vault(row: VaultPoseRow) -> PoseParams {
    PoseParams {
        slide: None,
        vault: Some(row),
    }
}

fn vaulting(progress: f32, landing: Vec3) -> Option<VaultViewInput> {
    Some(VaultViewInput {
        progress,
        path_s: PATH_S,
        landing: Some(landing),
    })
}

fn frame(
    movement_state: MovementStateKind,
    vault: Option<VaultViewInput>,
    aim_pitch_deg: f32,
) -> PoseFrame {
    PoseFrame {
        movement_state,
        vault,
        aim_pitch_deg,
        presented_eye: EYE,
        eye_height_above_feet: 10.0,
        offset_y: 0.0,
    }
}

/// One vault frame with the default still eye and landing.
fn vault_frame(progress: f32, aim_pitch_deg: f32) -> PoseFrame {
    frame(
        MovementStateKind::Vault,
        vaulting(progress, LANDING),
        aim_pitch_deg,
    )
}

fn normal_frame(aim_pitch_deg: f32) -> PoseFrame {
    frame(MovementStateKind::Normal, None, aim_pitch_deg)
}

/// Present a vault followed from its start up to `until`, returning each
/// frame's output.
fn climb(
    params: &PoseParams,
    state: &mut PoseState,
    frame_dt: f32,
    until: f32,
    aim_pitch_deg: f32,
) -> Vec<PoseOutput> {
    climb_scaled(params, state, frame_dt, until, aim_pitch_deg, 1.0)
}

fn climb_scaled(
    params: &PoseParams,
    state: &mut PoseState,
    frame_dt: f32,
    until: f32,
    aim_pitch_deg: f32,
    scale: f32,
) -> Vec<PoseOutput> {
    let frames = (until * PATH_S / frame_dt).ceil() as usize;
    (0..=frames)
        .map(|index| {
            let progress = (index as f32 * frame_dt / PATH_S).min(until);
            evaluate(
                Some(params),
                vault_frame(progress, aim_pitch_deg),
                state,
                frame_dt,
                scale,
            )
        })
        .collect()
}

fn released(
    params: &PoseParams,
    state: &mut PoseState,
    frame_dt: f32,
    frames: usize,
) -> Vec<PoseOutput> {
    (0..frames)
        .map(|_| evaluate(Some(params), normal_frame(0.0), state, frame_dt, 1.0))
        .collect()
}

fn pitches(outputs: &[PoseOutput]) -> Vec<f32> {
    outputs.iter().map(|output| output.pitch).collect()
}

fn steepest(values: &[f32]) -> f32 {
    values
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

fn assert_monotonic_toward_zero(values: &[f32], label: &str) {
    for pair in values.windows(2) {
        assert!(
            pair[1].abs() <= pair[0].abs() + 1.0e-6,
            "{label}: not monotonic toward neutral: {values:?}"
        );
    }
}

// --- Rate rows ---------------------------------------------------------------

#[test]
fn a_rate_row_holds_for_the_whole_state_and_every_exit_eases_monotonically_to_neutral() {
    // `Normal` stands for both an exit edge and a reconcile that writes it
    // with no edge: the row reads only the current state's kind.
    for exit in [
        MovementStateKind::Normal,
        MovementStateKind::Crouch,
        MovementStateKind::Dash,
        MovementStateKind::Swim,
        MovementStateKind::Vault,
    ] {
        let params = only_slide();
        let mut state = PoseState::default();
        let held: Vec<PoseOutput> = (0..600)
            .map(|_| {
                evaluate(
                    Some(&params),
                    frame(MovementStateKind::Slide, None, 0.0),
                    &mut state,
                    DT,
                    1.0,
                )
            })
            .collect();
        for output in &held[120..] {
            assert!((output.pitch + 2.0).abs() < EPS, "{exit:?}: {output:?}");
            assert!((output.roll - 1.0).abs() < EPS);
            assert!((output.eye_drop - 0.15).abs() < EPS);
            assert!((output.fov - 5.0).abs() < EPS);
        }
        let last = *held.last().unwrap();
        let exiting: Vec<PoseOutput> = (0..120)
            .map(|_| evaluate(Some(&params), frame(exit, None, 0.0), &mut state, DT, 1.0))
            .collect();
        for channel in [
            |o: &PoseOutput| o.pitch,
            |o: &PoseOutput| o.roll,
            |o: &PoseOutput| o.eye_drop,
            |o: &PoseOutput| o.fov,
        ] {
            let mut series = vec![channel(&last)];
            series.extend(exiting.iter().map(channel));
            assert_monotonic_toward_zero(&series, &format!("{exit:?}"));
            // One frame at the exit rate moves at most 1 − e^(−12/60) ≈ 18%.
            assert!(
                (series[1] - series[0]).abs() <= 0.19 * series[0].abs() + 1.0e-6,
                "{exit:?}: no step on the exit frame"
            );
            assert!(series.last().unwrap().abs() < 1.0e-4, "{exit:?}: neutral");
        }
    }
}

// Pin P1: a slide exit followed at once by a vault.
#[test]
fn a_slide_exit_into_a_vault_eases_both_rows_together_and_they_add() {
    let both = PoseParams {
        slide: Some(slide_row()),
        vault: Some(vault_row(-6.0, None)),
    };
    let mut state = PoseState::default();
    let mut slide_alone = PoseState::default();
    let mut vault_alone = PoseState::default();
    for _ in 0..120 {
        let slide = frame(MovementStateKind::Slide, None, 0.0);
        evaluate(Some(&both), slide, &mut state, DT, 1.0);
        evaluate(Some(&only_slide()), slide, &mut slide_alone, DT, 1.0);
        evaluate(
            Some(&only_vault(vault_row(-6.0, None))),
            slide,
            &mut vault_alone,
            DT,
            1.0,
        );
    }
    let mut series = vec![-2.0_f32];
    let mut overlapped = false;
    for index in 0..30 {
        let progress = index as f32 * DT / PATH_S;
        let vault = vault_frame(progress, 0.0);
        let sum = evaluate(Some(&both), vault, &mut state, DT, 1.0);
        let slide = evaluate(Some(&only_slide()), vault, &mut slide_alone, DT, 1.0);
        let lone = evaluate(
            Some(&only_vault(vault_row(-6.0, None))),
            vault,
            &mut vault_alone,
            DT,
            1.0,
        );
        assert!((sum.pitch - (slide.pitch + lone.pitch)).abs() < EPS);
        assert!((sum.roll - (slide.roll + lone.roll)).abs() < EPS);
        assert!((sum.fov - (slide.fov + lone.fov)).abs() < EPS);
        overlapped |= slide.pitch.abs() > 0.1 && lone.pitch.abs() > 0.1;
        series.push(sum.pitch);
    }
    assert!(overlapped, "both rows present during the handoff");
    // No frame moves more than the dev vault ease-in's steepest frame.
    assert!(steepest(&series) < 2.5, "{series:?}");
}

// Pin P17: frame-rate independence and zero-length frames.
#[test]
fn every_row_is_frame_rate_independent_and_a_zero_length_frame_holds_its_output() {
    let both = PoseParams {
        slide: Some(slide_row()),
        vault: Some(vault_row(-6.0, focus(0.7, 30.0))),
    };
    let at_time = |frame_dt: f32, seconds: f32| {
        let mut state = PoseState::default();
        let frames = (seconds / frame_dt).round() as usize;
        let mut output = PoseOutput::default();
        for index in 1..=frames {
            let t = index as f32 * frame_dt;
            // Slide for 0.2 s, then a vault that ends at 0.4 s: boundaries every
            // rate samples alike.
            let frame = if t <= 0.2 + 1.0e-4 {
                frame(MovementStateKind::Slide, None, 0.0)
            } else if t <= 0.4 + 1.0e-4 {
                vault_frame((t - 0.2) / PATH_S, 0.0)
            } else {
                normal_frame(0.0)
            };
            output = evaluate(Some(&both), frame, &mut state, frame_dt, 1.0);
        }
        output
    };
    for seconds in [0.2, 0.3, 0.4, 0.5] {
        let reference = at_time(1.0 / 240.0, seconds);
        for hz in [30.0, 60.0] {
            let output = at_time(1.0 / hz, seconds);
            for (a, b) in [
                (output.pitch, reference.pitch),
                (output.roll, reference.roll),
                (output.eye_drop, reference.eye_drop),
                (output.fov, reference.fov),
            ] {
                assert!((a - b).abs() < 2.0e-3, "{hz} Hz at {seconds} s: {a} vs {b}");
            }
        }
    }

    // A zero-length frame mid-slide, on a vault's end frame, and on a vault's
    // first frame holds every row's output.
    let mut state = PoseState::default();
    for _ in 0..30 {
        evaluate(
            Some(&both),
            frame(MovementStateKind::Slide, None, 0.0),
            &mut state,
            DT,
            1.0,
        );
    }
    let before = evaluate(
        Some(&both),
        frame(MovementStateKind::Slide, None, 0.0),
        &mut state,
        DT,
        1.0,
    );
    let held = evaluate(
        Some(&both),
        frame(MovementStateKind::Normal, None, 0.0),
        &mut state,
        0.0,
        1.0,
    );
    assert_eq!(held, before);

    let mut state = PoseState::default();
    let last = *climb(&both, &mut state, DT, 0.5, 0.0).last().unwrap();
    let end = evaluate(Some(&both), normal_frame(0.0), &mut state, 0.0, 1.0);
    assert_eq!(end, last, "a vault's zero-length end frame holds");

    // A vault's zero-length first frame holds, yet still takes the reference:
    // aim 20° on that frame, then 0° from the next, still focuses against 20°.
    let row = vault_row(0.0, focus(1.0, 80.0));
    let mut state = PoseState::default();
    let first = evaluate(
        Some(&only_vault(row)),
        vault_frame(0.0, 20.0),
        &mut state,
        0.0,
        1.0,
    );
    assert_eq!(first, PoseOutput::default());
    let mut output = PoseOutput::default();
    for index in 1..=12 {
        output = evaluate(
            Some(&only_vault(row)),
            vault_frame(index as f32 * DT / PATH_S, 0.0),
            &mut state,
            DT,
            1.0,
        );
    }
    let expected = line_deg(EYE, LANDING) - 20.0;
    assert!((output.pitch - expected).abs() < EPS, "{output:?}");

    // With `easeIn` 0 the weight is a function of progress alone, so only the
    // hold keeps a zero-length first frame, or one whose progress moved, still.
    let instant = PoseParams {
        slide: None,
        vault: Some(VaultPoseRow {
            ease_in: 0.0,
            ..vault_row(-6.0, focus(0.7, 30.0))
        }),
    };
    let mut state = PoseState::default();
    assert_eq!(
        evaluate(Some(&instant), vault_frame(0.3, 0.0), &mut state, 0.0, 1.0),
        PoseOutput::default()
    );
    let moving = evaluate(Some(&instant), vault_frame(0.3, 0.0), &mut state, DT, 1.0);
    assert!(moving.pitch < -6.0, "{moving:?}");
    let still = evaluate(Some(&instant), vault_frame(0.95, 0.0), &mut state, 0.0, 1.0);
    assert_eq!(still, moving, "progress moved on a zero-length frame");
}

// --- Vault row ---------------------------------------------------------------

// Regression: progress divided by `durationMs` while the path ran whole ticks
// past it, so the pose reached neutral before the pawn reached the top.
#[test]
fn the_vault_row_reaches_neutral_on_the_tick_the_path_reaches_the_top() {
    use postretro_physics::movement::{vault_clock, vault_path_ticks};
    let row = vault_row(-6.0, focus(0.7, 30.0));
    for hz in [30.0_f32, 60.0, 144.0, 240.0] {
        let tick_s = 1.0 / hz;
        let step_ms = tick_s * 1000.0;
        let ticks = vault_path_ticks(420.0, step_ms);
        let at_top =
            VaultViewInput::presented(vault_clock(ticks, step_ms), 420.0, tick_s, 1.0, None);
        assert_eq!(
            at_top.progress, 1.0,
            "{hz} Hz: neutral as the top is reached"
        );
        assert_eq!(vault_row::envelope(&row, at_top.progress), 0.0);
        let before =
            VaultViewInput::presented(vault_clock(ticks - 1, step_ms), 420.0, tick_s, 1.0, None);
        assert!(
            vault_row::envelope(&row, before.progress) > 0.0,
            "{hz} Hz: still easing out a tick before the top"
        );
    }
}

#[test]
fn presented_progress_trails_the_latest_tick_by_the_unpresented_share() {
    use postretro_physics::movement::{vault_clock, vault_path_ms};
    let tick_s = 1.0 / 60.0;
    let step_ms = tick_s * 1000.0;
    // 420 ms runs 26 whole ticks at 60 Hz.
    let path_ms = vault_path_ms(420.0, step_ms);
    assert_eq!(path_ms, vault_clock(26, step_ms));
    // The entry tick leaves one step elapsed; at alpha 0 the camera still
    // shows the pose before it.
    assert_eq!(
        VaultViewInput::presented(step_ms, 420.0, tick_s, 0.0, None).progress,
        0.0
    );
    let half = VaultViewInput::presented(step_ms, 420.0, tick_s, 0.5, None).progress;
    assert!((half - 0.5 * step_ms / path_ms).abs() < 1.0e-6);
    let top = VaultViewInput::presented(path_ms + step_ms, 420.0, tick_s, 1.0, Some(LANDING));
    assert_eq!(top.progress, 1.0);
    assert_eq!(top.path_s, path_ms / 1000.0);
    assert_eq!(top.landing, Some(LANDING));
}

#[test]
fn an_early_end_releases_the_last_presented_pose_focus_included_monotonically() {
    // A blocked path, death and a reconcile that ends the vault all present
    // as the vault input going away.
    let params = only_vault(vault_row(-6.0, focus(0.7, 30.0)));
    let mut state = PoseState::default();
    let held = climb(&params, &mut state, DT, 0.5, 0.0);
    let last = *held.last().unwrap();
    let focus_deg = 0.7 * (line_deg(EYE, LANDING) - (-6.0));
    assert!((last.pitch - (-6.0 + focus_deg)).abs() < EPS, "{last:?}");
    let release = released(&params, &mut state, DT, 40);
    let release_s = (PATH_S * 0.2).max(MIN_RELEASE_S);
    let release_frames = (release_s / DT).ceil() as usize;
    for (label, channel) in [
        (
            "pitch",
            (|o: &PoseOutput| o.pitch) as fn(&PoseOutput) -> f32,
        ),
        ("roll", |o: &PoseOutput| o.roll),
        ("fov", |o: &PoseOutput| o.fov),
    ] {
        let mut series = vec![channel(&last)];
        series.extend(release.iter().map(channel));
        assert_monotonic_toward_zero(&series, label);
        // A smoothstep release starts gently.
        assert!(
            (series[1] - series[0]).abs() < 0.5 * steepest(&series),
            "{label}: no step on the end frame"
        );
        assert!(
            series[release_frames + 1].abs() < 1.0e-6,
            "{label}: neutral"
        );
        assert!(
            series[release_frames / 2].abs() > 0.2 * series[0].abs(),
            "{label}: a release, not a snap"
        );
    }
    // The release fades the values it last presented: focus is not
    // re-evaluated once the geometry is gone.
    for output in &release[..release_frames] {
        if output.roll.abs() > 1.0e-3 {
            assert!((output.pitch / output.roll - last.pitch / last.roll).abs() < 1.0e-3);
        }
    }

    // A short climb still releases over at least the minimum.
    let short = PoseParams {
        slide: None,
        vault: Some(VaultPoseRow {
            ease_out: 0.0,
            ..vault_row(-6.0, None)
        }),
    };
    let mut state = PoseState::default();
    climb(&short, &mut state, DT, 1.0, 0.0);
    let release = pitches(&released(&short, &mut state, DT, 10));
    assert!(release[0] < -5.0, "no drop at the end: {release:?}");
    let minimum_frames = (MIN_RELEASE_S / DT).ceil() as usize;
    assert!(release[minimum_frames - 1].abs() < 1.0e-6, "{release:?}");
    assert!(release[minimum_frames / 2] < -1.0, "a release, not a snap");
}

// Pin P3: a vault that ends while an earlier release still runs.
#[test]
fn a_vault_ending_during_a_release_releases_the_blended_pose_it_presented() {
    let row = vault_row(0.0, focus(1.0, 60.0));
    let params = only_vault(row);
    let mut state = PoseState::default();
    climb(&params, &mut state, DT, 0.5, 0.0);
    released(&params, &mut state, DT, 1);
    // Vault B at a higher ledge starts while A's release holds most of A.
    let high = Vec3::new(0.6, 1.4, 0.0);
    evaluate(
        Some(&params),
        frame(MovementStateKind::Vault, vaulting(0.0, high), 0.0),
        &mut state,
        DT,
        1.0,
    );
    let last = evaluate(
        Some(&params),
        frame(MovementStateKind::Vault, vaulting(DT / PATH_S, high), 0.0),
        &mut state,
        DT,
        1.0,
    );
    let blended = last.pitch;
    // Both vaults roll 2°, so the roll reads the presented weight back.
    let weight = last.roll / 2.0;
    let values = blended / weight;
    let a_only = line_deg(EYE, LANDING);
    let b_only = line_deg(EYE, high);
    assert!(
        values < b_only - 0.5 && values > a_only + 0.5,
        "B ends mid-blend: {values} between {a_only} and {b_only}"
    );
    // B aborts: its release starts from that blend, with no step.
    let release = released(&params, &mut state, DT, 20);
    for output in &release {
        if output.roll.abs() > 1.0e-3 {
            assert!(
                (output.pitch / output.roll - blended / last.roll).abs() < 1.0e-3,
                "the release fades the blended values"
            );
        }
    }
    let mut series = vec![blended];
    series.extend(pitches(&release));
    assert!(
        (series[1] - series[0]).abs() < 0.5 * steepest(&series),
        "the release starts gently: {series:?}"
    );
    assert_monotonic_toward_zero(&series, "B's release");
}

// Pin P10: back-to-back vaults at different ledges, and a rewind. The overlap
// is A's release and B's ease-in; neither steps, so their blend may move no
// faster than those two together, and B's first frame must not show the
// w_A · |v_A − v_B| step presenting B's values alone would.
#[test]
fn back_to_back_vaults_at_different_ledges_and_a_rewind_never_step() {
    let params = only_vault(vault_row(0.0, focus(0.7, 40.0)));
    let high = Vec3::new(0.6, 1.3, 0.0);
    let b_frames = |state: &mut PoseState, from: f32, landing: Vec3| -> Vec<f32> {
        (0..20)
            .map(|index| {
                let progress = from + index as f32 * DT / PATH_S;
                evaluate(
                    Some(&params),
                    frame(MovementStateKind::Vault, vaulting(progress, landing), 0.0),
                    state,
                    DT,
                    1.0,
                )
                .pitch
            })
            .collect()
    };
    // The parts alone: A releasing from its hold, B easing in from neutral.
    let mut alone = PoseState::default();
    let mut a_release = pitches(&climb(&params, &mut alone, DT, 0.6, 0.0));
    let a_held = *a_release.last().unwrap();
    a_release = vec![a_held];
    a_release.extend(pitches(&released(&params, &mut alone, DT, 20)));
    let mut alone = PoseState::default();
    let mut b_ease_in = vec![0.0];
    b_ease_in.extend(b_frames(&mut alone, 0.0, high));
    let b_held = *b_ease_in.last().unwrap();
    let allowed = steepest(&a_release) + steepest(&b_ease_in) + 1.0e-3;
    assert!(
        (a_held - b_held).abs() > 5.0,
        "the ledges differ: {a_held} vs {b_held}"
    );

    // A at its hold, then B with no `Normal` frame between them: B shows only
    // as a progress drop.
    let mut state = PoseState::default();
    let mut series = pitches(&climb(&params, &mut state, DT, 0.6, 0.0));
    series.extend(b_frames(&mut state, 0.0, high));
    let first = series.len() - 20;
    assert!(steepest(&series) <= allowed, "{series:?} vs {allowed}");
    assert!(
        (series[first] - a_held).abs() < 0.25 * (a_held - b_held).abs(),
        "no step on B's first frame: {a_held} → {}",
        series[first]
    );
    assert!((series.last().unwrap() - b_held).abs() < 0.05);

    // A reconcile rewinds the vault past 5%: the same rule.
    let mut state = PoseState::default();
    let mut series = pitches(&climb(&params, &mut state, DT, 0.6, 0.0));
    series.extend(b_frames(&mut state, 0.3, LANDING));
    assert!(steepest(&series) <= allowed, "{series:?}");
}

#[test]
fn a_vault_that_appears_mid_climb_eases_in_from_neutral_with_its_focus() {
    // A reconcile starts a vault the client never predicted, half way up; or
    // a descriptor change clears view-feel state mid-vault (pins P6, P18).
    // The clear and the join on one frame are composed in `frame_eye`; here
    // the row sees only a vault its state was not following.
    let params = only_vault(vault_row(-6.0, focus(0.7, 80.0)));
    {
        let mut state = PoseState::default();
        let series: Vec<f32> = (0..7)
            .map(|index| {
                let progress = 0.5 + index as f32 * DT / PATH_S;
                // The join frame aims 10° up, then the player looks level.
                let aim = if index == 0 { 10.0 } else { 0.0 };
                evaluate(
                    Some(&params),
                    vault_frame(progress, aim),
                    &mut state,
                    DT,
                    1.0,
                )
                .pitch
            })
            .collect();
        // After the ease-in: the literal plus focus against the join frame's aim.
        let ease_in_frames = (0.15 * PATH_S / DT).ceil() as usize;
        let expected = -6.0 + 0.7 * (line_deg(EYE, LANDING) - (10.0 - 6.0));
        // The join frame counts its own time on a smoothstep ramp, and no
        // frame moves further than that ramp's steepest (1.5 × dt / ease-in).
        let steepest_ramp = 1.5 * DT / (0.15 * PATH_S) * expected.abs();
        assert!(
            series[0].abs() < 0.25 * expected.abs(),
            "no snap to the held pose: {series:?}"
        );
        let mut with_neutral = vec![0.0];
        with_neutral.extend(&series);
        for pair in with_neutral.windows(2) {
            assert!(pair[1] <= pair[0] + 1.0e-6, "eases in: {series:?}");
            assert!(
                (pair[1] - pair[0]).abs() <= steepest_ramp,
                "no step: {series:?}"
            );
        }
        assert!(
            (series[ease_in_frames] - expected).abs() < EPS,
            "{series:?} vs {expected}"
        );
    }
}

// --- Ledge focus -------------------------------------------------------------

#[test]
fn ledge_focus_pulls_strength_of_the_gap_only_toward_a_line_below_the_reference() {
    let row_focus = LedgeFocus {
        strength: 0.5,
        max_pitch: 30.0,
    };
    let line = line_deg(EYE, LANDING);
    // Above the line: half the gap, toward it.
    let offset = vault_row::ledge_focus(&row_focus, EYE, LANDING, 0.0);
    assert!((offset - 0.5 * line).abs() < EPS, "{offset} vs {line}");
    // At and below the line: nothing.
    assert_eq!(vault_row::ledge_focus(&row_focus, EYE, LANDING, line), 0.0);
    assert_eq!(
        vault_row::ledge_focus(&row_focus, EYE, LANDING, line - 10.0),
        0.0
    );
    // A landing spot at or above the eye never pulls, at any aim.
    for landing_y in [EYE.y, EYE.y + 0.4] {
        for reference in [-80.0, 0.0, 80.0] {
            let landing = Vec3::new(0.6, landing_y, 0.0);
            assert_eq!(
                vault_row::ledge_focus(&row_focus, EYE, landing, reference),
                0.0
            );
        }
    }
    // Capped at maxPitch.
    let strong = LedgeFocus {
        strength: 1.0,
        max_pitch: 12.0,
    };
    assert_eq!(vault_row::ledge_focus(&strong, EYE, LANDING, 40.0), -12.0);
    // The final tick, the eye straight above the landing spot: finite, capped.
    let above = Vec3::new(LANDING.x, LANDING.y + 1.5, LANDING.z);
    let last = vault_row::ledge_focus(&strong, above, LANDING, 0.0);
    assert_eq!(last, -12.0);
    let loose = LedgeFocus {
        strength: 1.0,
        max_pitch: 89.0,
    };
    let last = vault_row::ledge_focus(&loose, above, LANDING, 0.0);
    assert!(last.is_finite() && last > -89.0 && last < -60.0, "{last}");

    // Through the row at full weight and scale: focus adds to the literal, and
    // an impulse or another row adds on top in composition (`frame_eye`).
    let params = only_vault(vault_row(0.0, focus(0.5, 30.0)));
    let mut state = PoseState::default();
    let held = *climb(&params, &mut state, DT, 0.5, 0.0).last().unwrap();
    assert!((held.pitch - 0.5 * line).abs() < EPS, "{held:?}");
    let mut state = PoseState::default();
    let below = *climb(&params, &mut state, DT, 0.5, line - 5.0)
        .last()
        .unwrap();
    assert!(below.pitch.abs() < EPS, "{below:?}");
}

#[test]
fn a_literal_pitch_counts_toward_the_reference_and_focus_never_passes_the_line() {
    let line = line_deg(EYE, LANDING);
    // A literal that already puts the reference below the line adds no focus.
    let params = only_vault(vault_row(line - 2.0, focus(1.0, 80.0)));
    let mut state = PoseState::default();
    let held = *climb(&params, &mut state, DT, 0.5, 0.0).last().unwrap();
    assert!((held.pitch - (line - 2.0)).abs() < EPS, "{held:?}");
    // A literal above the line: reference + offset approaches it, never past.
    for strength in [0.25, 0.5, 1.0] {
        let params = only_vault(vault_row(-10.0, focus(strength, 80.0)));
        let mut state = PoseState::default();
        let view = climb(&params, &mut state, DT, 0.5, 5.0)
            .last()
            .unwrap()
            .pitch
            + 5.0;
        assert!(
            view >= line - EPS && view < -10.0 + 5.0,
            "{strength}: {view}"
        );
        if strength == 1.0 {
            assert!((view - line).abs() < EPS);
        }
    }
}

// Pins P5, P14, P15: mouse pitch maps 1:1 from the frame after the reference.
#[test]
fn mouse_pitch_maps_one_to_one_after_the_reference_through_the_climb_and_release() {
    for strength in [0.0, 0.4, 1.0] {
        let params = only_vault(vault_row(0.0, focus(strength, 30.0)));
        // Two runs differ only in aim after the capture frame.
        let run = |aims: &dyn Fn(usize) -> f32| -> Vec<f32> {
            let mut state = PoseState::default();
            let mut view = Vec::new();
            for index in 0..=40 {
                let aim = aims(index);
                let progress = index as f32 * DT / PATH_S;
                let frame = if progress < 0.7 {
                    vault_frame(progress, aim)
                } else {
                    normal_frame(aim)
                };
                view.push(aim + evaluate(Some(&params), frame, &mut state, DT, 1.0).pitch);
            }
            view
        };
        let level = run(&|_| 0.0);
        // Look up then down past the line (and mid-climb looking down adds).
        let looking = |index: usize| {
            if index == 0 {
                0.0
            } else {
                40.0 * (index as f32 * 0.4).sin() - 30.0
            }
        };
        let moved = run(&looking);
        for (index, (a, b)) in level.iter().zip(&moved).enumerate() {
            let aim = looking(index);
            assert!(
                ((b - a) - aim).abs() < EPS,
                "strength {strength}, frame {index}: view moved {} for aim {aim}",
                b - a
            );
        }
    }

    // The capture frame's own look moves the reference with it (P15): with
    // zero easeIn and a reference above the line, that frame moves the view
    // by (1 − strength) of the flick; the next frame's flick moves it 1:1.
    let row = VaultPoseRow {
        ease_in: 0.0,
        ..vault_row(0.0, focus(0.5, 80.0))
    };
    let params = only_vault(row);
    let view_at = |capture_aim: f32, next_aim: f32| -> (f32, f32) {
        let mut state = PoseState::default();
        let first = capture_aim
            + evaluate(
                Some(&params),
                vault_frame(0.3, capture_aim),
                &mut state,
                DT,
                1.0,
            )
            .pitch;
        let second = next_aim
            + evaluate(
                Some(&params),
                vault_frame(0.3 + DT / PATH_S, next_aim),
                &mut state,
                DT,
                1.0,
            )
            .pitch;
        (first, second)
    };
    let (still, _) = view_at(0.0, 0.0);
    let (flicked, after) = view_at(10.0, 10.0);
    assert!((flicked - still - 5.0).abs() < EPS, "{still} → {flicked}");
    let (_, next_flick) = view_at(10.0, 20.0);
    assert!(
        (next_flick - after - 10.0).abs() < EPS,
        "{after} → {next_flick}"
    );

    // A new vault, or a rewind past 5%, takes a new reference.
    let params = only_vault(vault_row(0.0, focus(1.0, 80.0)));
    let mut state = PoseState::default();
    climb(&params, &mut state, DT, 0.5, 0.0);
    let mut output = PoseOutput::default();
    for index in 0..8 {
        let progress = 0.2 + index as f32 * DT / PATH_S;
        output = evaluate(
            Some(&params),
            vault_frame(progress, 15.0),
            &mut state,
            DT,
            1.0,
        );
    }
    assert!(
        (output.pitch - (line_deg(EYE, LANDING) - 15.0)).abs() < EPS,
        "the rewind's reference is its own frame's aim: {output:?}"
    );
}

#[test]
fn a_full_climb_with_focus_starts_and_ends_at_neutral() {
    let params = only_vault(vault_row(-6.0, focus(0.7, 30.0)));
    let mut state = PoseState::default();
    let series = pitches(&climb(&params, &mut state, DT, 1.0, 0.0));
    assert_eq!(series[0], 0.0);
    assert!(series.last().unwrap().abs() < 1.0e-6);
    assert!(
        released(&params, &mut state, DT, 20)
            .iter()
            .all(|output| output.pitch.abs() < 1.0e-6),
        "nothing left to release"
    );
}

// Pin P16: scale 0 as a vault starts, restored mid-climb.
#[test]
fn a_scale_restored_mid_climb_shows_the_pose_against_the_entry_reference() {
    let params = only_vault(vault_row(-6.0, focus(0.7, 80.0)));
    let mut scaled = PoseState::default();
    let mut unscaled = PoseState::default();
    for index in 0..=12 {
        let progress = index as f32 * DT / PATH_S;
        // Entry aims 12° up; later frames look level.
        let aim = if index == 0 { 12.0 } else { 0.0 };
        let frame = vault_frame(progress, aim);
        let hidden = evaluate(Some(&params), frame, &mut scaled, DT, 0.0);
        assert_eq!(hidden, PoseOutput::default());
        evaluate(Some(&params), frame, &mut unscaled, DT, 1.0);
    }
    let frame = vault_frame(13.0 * DT / PATH_S, 0.0);
    let restored = evaluate(Some(&params), frame, &mut scaled, DT, 1.0);
    let reference = evaluate(Some(&params), frame, &mut unscaled, DT, 1.0);
    assert!((restored.pitch - reference.pitch).abs() < EPS);
    let expected = -6.0 + 0.7 * (line_deg(EYE, LANDING) - (12.0 - 6.0));
    assert!((restored.pitch - expected).abs() < EPS, "{restored:?}");
}

// Pin P21: an abort and a new vault inside one frame, dropping under 5%.
#[test]
fn a_progress_drop_under_the_restart_share_keeps_the_reference_and_re_aims() {
    let params = only_vault(vault_row(0.0, focus(1.0, 80.0)));
    let mut state = PoseState::default();
    climb(&params, &mut state, DT, 0.5, 20.0);
    let moved = Vec3::new(0.4, 0.8, 0.3);
    let output = evaluate(
        Some(&params),
        frame(MovementStateKind::Vault, vaulting(0.47, moved), 0.0),
        &mut state,
        DT,
        1.0,
    );
    let expected = line_deg(EYE, moved) - 20.0;
    assert!(
        (output.pitch - expected).abs() < EPS,
        "{output:?} vs {expected}"
    );
}

// A mover vault whose mover has no presented pose for a frame or more: the
// focus it last showed holds, and an end then releases it.
#[test]
fn a_landing_spot_gone_mid_vault_holds_the_focus_and_releases_it() {
    let params = only_vault(vault_row(0.0, focus(0.7, 80.0)));
    let mut state = PoseState::default();
    let held = climb(&params, &mut state, DT, 0.5, 0.0)
        .last()
        .unwrap()
        .pitch;
    assert!(held < -10.0, "{held}");
    let mut series = vec![held];
    for index in 1..=3 {
        let progress = 0.5 + index as f32 * DT / PATH_S;
        let frame = frame(
            MovementStateKind::Vault,
            Some(VaultViewInput {
                progress,
                path_s: PATH_S,
                landing: None,
            }),
            0.0,
        );
        series.push(evaluate(Some(&params), frame, &mut state, DT, 1.0).pitch);
    }
    for pitch in &series[1..] {
        assert!((pitch - held).abs() < EPS, "the focus holds: {series:?}");
    }
    // The vault then breaks off: the release carries that focus.
    let release = pitches(&released(&params, &mut state, DT, 20));
    series.extend(&release);
    assert_monotonic_toward_zero(&series, "release with the held focus");
    assert!(
        (series[4] - held).abs() < 0.5 * steepest(&series),
        "{series:?}"
    );

    // A pose that returns re-aims from the geometry it finds.
    let mut state = PoseState::default();
    climb(&params, &mut state, DT, 0.4, 0.0);
    let none = Some(VaultViewInput {
        progress: 0.42,
        path_s: PATH_S,
        landing: None,
    });
    evaluate(
        Some(&params),
        frame(MovementStateKind::Vault, none, 0.0),
        &mut state,
        DT,
        1.0,
    );
    let back = evaluate(Some(&params), vault_frame(0.44, 0.0), &mut state, DT, 1.0);
    assert!((back.pitch - 0.7 * line_deg(EYE, LANDING)).abs() < EPS);
}

// Focus is pitch-only: the bearing to the landing spot never enters it.
#[test]
fn ledge_focus_is_the_same_at_any_bearing_to_the_landing_spot() {
    let row_focus = LedgeFocus {
        strength: 0.6,
        max_pitch: 50.0,
    };
    let ahead = vault_row::ledge_focus(&row_focus, EYE, LANDING, 0.0);
    for bearing in [0.5_f32, 1.6, 3.1, -2.2] {
        let landing = EYE + glam::Quat::from_rotation_y(bearing) * (LANDING - EYE);
        let offset = vault_row::ledge_focus(&row_focus, EYE, landing, 0.0);
        assert!(
            (offset - ahead).abs() < EPS,
            "{bearing}: {offset} vs {ahead}"
        );
    }
}

// Pin P20: the line is measured from the eye after every row's drop.
#[test]
fn the_ledge_line_is_measured_from_the_eye_after_every_rows_drop() {
    let line_at = |eye_drop: f32| {
        let row = VaultPoseRow {
            channels: channels(0.0, 0.0, eye_drop, 0.0),
            ..vault_row(0.0, focus(1.0, 80.0))
        };
        let mut state = PoseState::default();
        *climb(&only_vault(row), &mut state, DT, 0.5, 0.0)
            .last()
            .unwrap()
    };
    // A lower eye sits closer to a landing below it, so its line is shallower.
    let level = line_at(0.0);
    let dropped = line_at(0.3);
    assert!((dropped.eye_drop - 0.3).abs() < EPS);
    let expected = line_deg(EYE - Vec3::Y * 0.3, LANDING);
    assert!((dropped.pitch - expected).abs() < EPS, "{dropped:?}");
    assert!(dropped.pitch > level.pitch + 1.0);
    // A ledge just under the undropped eye is above an eye dropped under it.
    let tall = Vec3::new(0.6, EYE.y - 0.1, 0.0);
    let row = VaultPoseRow {
        channels: channels(0.0, 0.0, 0.3, 0.0),
        ..vault_row(0.0, focus(1.0, 80.0))
    };
    let mut state = PoseState::default();
    let mut output = PoseOutput::default();
    for index in 0..=12 {
        output = evaluate(
            Some(&only_vault(row)),
            frame(
                MovementStateKind::Vault,
                vaulting(index as f32 * DT / PATH_S, tall),
                0.0,
            ),
            &mut state,
            DT,
            1.0,
        );
    }
    assert!(output.pitch.abs() < EPS, "{output:?}");
}

// --- Scale and lifecycle -------------------------------------------------------

// Pin P4: the view-feel scale multiplies output only.
#[test]
fn the_view_feel_scale_scales_every_row_without_freezing_it() {
    // The vault row drops the eye too, so a scaled drop must not move its line.
    let dropping_vault = VaultPoseRow {
        channels: channels(-6.0, 2.0, 0.3, -3.0),
        ..vault_row(-6.0, focus(0.7, 80.0))
    };
    let both = PoseParams {
        slide: Some(slide_row()),
        vault: Some(dropping_vault),
    };
    for scale in [1.0, 0.5, 0.0] {
        let mut state = PoseState::default();
        let mut full = PoseState::default();
        for _ in 0..120 {
            let slide = frame(MovementStateKind::Slide, None, 0.0);
            let a = evaluate(Some(&both), slide, &mut state, DT, scale);
            let b = evaluate(Some(&both), slide, &mut full, DT, 1.0);
            assert!((a.eye_drop - b.eye_drop * scale).abs() < 1.0e-6);
            assert!((a.fov - b.fov * scale).abs() < 1.0e-5);
        }
        let mut state = PoseState::default();
        let mut full = PoseState::default();
        let a = climb_scaled(&both, &mut state, DT, 0.5, 0.0, scale);
        let b = climb(&both, &mut full, DT, 0.5, 0.0);
        for (a, b) in a.iter().zip(&b) {
            assert!(
                (a.pitch - b.pitch * scale).abs() < 1.0e-4,
                "{scale}: {a:?} vs {b:?}"
            );
            assert!((a.roll - b.roll * scale).abs() < 1.0e-5);
            assert!((a.eye_drop - b.eye_drop * scale).abs() < 1.0e-5);
        }
    }

    // Reduced motion mid-hold, restored: resumes from the live weight.
    let mut state = PoseState::default();
    for _ in 0..60 {
        let output = evaluate(
            Some(&both),
            frame(MovementStateKind::Slide, None, 0.0),
            &mut state,
            DT,
            0.0,
        );
        assert_eq!(output, PoseOutput::default());
    }
    let restored = evaluate(
        Some(&both),
        frame(MovementStateKind::Slide, None, 0.0),
        &mut state,
        0.0,
        1.0,
    );
    assert!((restored.eye_drop - 0.15).abs() < EPS);

    // Scale 0 through a vault's end, restored mid-release: the rest of that
    // release, focus included.
    let mut hidden = PoseState::default();
    let mut shown = PoseState::default();
    let vault_only = only_vault(dropping_vault);
    let frames = (0.5 * PATH_S / DT).ceil() as usize;
    for index in 0..=frames {
        let frame = vault_frame((index as f32 * DT / PATH_S).min(0.5), 0.0);
        evaluate(Some(&vault_only), frame, &mut hidden, DT, 0.0);
        evaluate(Some(&vault_only), frame, &mut shown, DT, 1.0);
    }
    for _ in 0..2 {
        evaluate(Some(&vault_only), normal_frame(0.0), &mut hidden, DT, 0.0);
        evaluate(Some(&vault_only), normal_frame(0.0), &mut shown, DT, 1.0);
    }
    let a = evaluate(Some(&vault_only), normal_frame(0.0), &mut hidden, DT, 1.0);
    let b = evaluate(Some(&vault_only), normal_frame(0.0), &mut shown, DT, 1.0);
    assert!(
        a.pitch < -1.0 && (a.pitch - b.pitch).abs() < EPS,
        "{a:?} vs {b:?}"
    );
}

#[test]
fn a_missing_block_or_row_presents_nothing_and_keeps_no_state() {
    let mut state = PoseState::default();
    climb(&only_vault(vault_row(-6.0, None)), &mut state, DT, 0.5, 0.0);
    assert_eq!(
        evaluate(None, vault_frame(0.5, 0.0), &mut state, DT, 1.0),
        PoseOutput::default()
    );
    assert_eq!(state, PoseState::default());
    for _ in 0..30 {
        evaluate(
            Some(&only_slide()),
            frame(MovementStateKind::Slide, None, 0.0),
            &mut state,
            DT,
            1.0,
        );
    }
    let empty = PoseParams::default();
    assert_eq!(
        evaluate(
            Some(&empty),
            frame(MovementStateKind::Slide, None, 0.0),
            &mut state,
            DT,
            1.0
        ),
        PoseOutput::default()
    );
    assert_eq!(state, PoseState::default());
}
