// The vault row of `viewFeel.pose`: weighted by the climb's own progress, with
// an optional entry-referenced ledge focus.
// See: context/lib/movement.md §1

use glam::{Vec2, Vec3};
use postretro_foundation::{LedgeFocus, VaultPoseRow};
use postretro_physics::movement::vault_path_ms;

use super::PoseValues;

/// Shortest ease back to neutral, seconds, after a vault ends with weight
/// left: an early end, or the end of a vault whose `easeOut` is 0. A longer
/// one comes from the climb's time × `easeOut`.
pub(super) const MIN_RELEASE_S: f32 = 0.1;

/// Progress drop, as a share of the vault, that reads as a new vault rather
/// than a reconcile nudging the current one.
const RESTART_PROGRESS_DROP: f32 = 0.05;

/// Floor on the eye's horizontal distance to the landing spot, metres. As the
/// eye passes over the landing spot the distance goes to 0 and the ledge line
/// toward −90°; the floor bounds it (−71.6° at an eye 1.5 m above the spot).
/// With a nonzero `easeOut` the row is also easing out there; with 0 the floor
/// is the only bound short of `maxPitch`.
pub(super) const LEDGE_FOCUS_MIN_HORIZONTAL_M: f32 = 0.5;

/// The local pawn's vault this frame, as the camera presents it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VaultViewInput {
    /// Presented share of the vault, 0..=1.
    pub(crate) progress: f32,
    /// How long the climb runs, seconds: `durationMs` rounded up to whole
    /// ticks, as the movement path runs it.
    pub(crate) path_s: f32,
    /// The landing spot in world space at this frame's interpolation share,
    /// through the carrying mover's presented pose. `None` when the mover has
    /// no presented pose; the row then holds the focus it last showed.
    pub(crate) landing: Option<Vec3>,
}

impl VaultViewInput {
    /// The presented progress of a vault whose latest tick left `elapsed_ms`
    /// with `durationMs` `duration_ms`: the camera shows the pose `alpha` of
    /// the way from the previous tick to that one, so the vault it shows is
    /// `(1 − alpha)` of a tick behind it. Progress is over the path's own
    /// length (`vault_path_ms`), so it reaches 1 as the pawn reaches the top.
    pub(crate) fn presented(
        elapsed_ms: f32,
        duration_ms: f32,
        tick_s: f32,
        alpha: f32,
        landing: Option<Vec3>,
    ) -> Self {
        let step_ms = tick_s * 1000.0;
        let path_ms = vault_path_ms(duration_ms, step_ms);
        let presented_ms = elapsed_ms - (1.0 - alpha.clamp(0.0, 1.0)) * step_ms;
        let progress = if path_ms > 0.0 {
            (presented_ms / path_ms).clamp(0.0, 1.0)
        } else {
            1.0
        };
        Self {
            progress,
            path_s: path_ms / 1000.0,
            landing,
        }
    }
}

/// Render-side integrator for the vault row. Not tick state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct VaultRowState {
    /// Weight presented last frame, before the view-feel scale.
    weight: f32,
    /// Values presented last frame, unweighted and unscaled: a release fades
    /// these, since the vault's geometry is gone once it ends.
    values: PoseValues,
    /// Progress last frame, while vaulting.
    progress: Option<f32>,
    /// The weight and values a release started from, and how long it has run.
    release_from: f32,
    release_values: PoseValues,
    release_elapsed_s: f32,
    release_s: f32,
    /// Time since the pose started following this vault, seconds: the ease-in
    /// from neutral a vault that appears already in progress takes.
    join_elapsed_s: f32,
    /// Ledge focus reference, degrees: the aim pitch the pose's first frame on
    /// this vault rendered, plus the row's literal pitch.
    pub(super) reference_deg: f32,
    /// Ledge focus this vault last presented, degrees, unweighted: held while
    /// its landing spot has no presented pose.
    pub(super) focus_deg: f32,
}

/// The vault row's timing for one frame, before its values are known: the
/// live vault's and the release's weights, unscaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct VaultWeights {
    live: f32,
    release: f32,
    /// A zero-length frame: weights and values hold.
    hold: bool,
}

/// Weight of the held pose at `progress`: up over the first `ease_in` share,
/// held, down over the last `ease_out` share to exactly 0 at the end. Both
/// ramps are smoothsteps, so the pose starts and settles with no velocity
/// step. A zero `ease_in` starts at full weight; a zero `ease_out` holds full
/// weight to the end, and the release that follows the vault's end
/// ([`MIN_RELEASE_S`]) eases it back.
pub(super) fn envelope(row: &VaultPoseRow, progress: f32) -> f32 {
    let up = if row.ease_in > 0.0 {
        smoothstep(progress / row.ease_in)
    } else {
        1.0
    };
    let down = if row.ease_out > 0.0 {
        smoothstep((1.0 - progress) / row.ease_out)
    } else {
        1.0
    };
    up.min(down)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Advance the row's timing by one frame and take a ledge-focus reference when
/// the pose starts following a vault.
///
/// While `vaulting` is set the live weight is the envelope at the presented
/// progress, so it tracks the climb itself rather than a timer. Every other
/// way out — an abort, death, a reconcile that ends the vault with no edge —
/// starts a release: the presented weight and values ease to 0 on a
/// smoothstep over the climb's time × `easeOut`, at least [`MIN_RELEASE_S`].
/// A descriptor refresh clears the state instead (`clear_state_effects`).
///
/// A vault the pose was not following — one that starts, one a reconcile or
/// a mid-vault reset shows already in progress, or a rewind past
/// [`RESTART_PROGRESS_DROP`] read as a new vault — eases in from neutral over
/// the climb's time × `easeIn`, and takes the reference from this frame's
/// `aim_pitch_deg`. The ramp counts this frame, so it never holds back a vault
/// that started this frame. A zero-length frame still marks the join and takes
/// the reference, but holds the weights.
pub(super) fn advance(
    row: &VaultPoseRow,
    vaulting: Option<VaultViewInput>,
    aim_pitch_deg: f32,
    state: &mut VaultRowState,
    frame_dt: f32,
) -> VaultWeights {
    let rewound = match (vaulting, state.progress) {
        (Some(input), Some(last)) => input.progress < last - RESTART_PROGRESS_DROP,
        _ => false,
    };
    let joined = rewound || (vaulting.is_some() && state.progress.is_none());
    let ended = vaulting.is_none() && state.progress.is_some();
    if rewound || ended {
        state.release_from = state.weight;
        state.release_values = state.values;
        state.release_elapsed_s = 0.0;
    }
    if joined {
        state.join_elapsed_s = 0.0;
        state.reference_deg = aim_pitch_deg + row.channels.pitch;
        state.focus_deg = 0.0;
    }
    if let Some(input) = vaulting {
        state.release_s = (input.path_s * row.ease_out).max(MIN_RELEASE_S);
    }
    state.progress = vaulting.map(|input| input.progress);
    let frame_dt = frame_dt.max(0.0);
    state.release_elapsed_s += frame_dt;
    state.join_elapsed_s += frame_dt;
    let release = if state.release_s > 0.0 {
        state.release_from * (1.0 - smoothstep(state.release_elapsed_s / state.release_s))
    } else {
        0.0
    };
    let live = match vaulting {
        Some(input) => {
            let ease_in_s = input.path_s * row.ease_in;
            let join = if ease_in_s > 0.0 {
                smoothstep(state.join_elapsed_s / ease_in_s)
            } else {
                1.0
            };
            envelope(row, input.progress).min(join)
        }
        None => 0.0,
    };
    VaultWeights {
        live,
        release,
        hold: frame_dt == 0.0,
    }
}

/// The row's presented weight and values this frame, recorded for the next.
///
/// A release and a live vault overlapping present the larger weight with the
/// two value sets blended by their weights, so a re-vault at a different ledge
/// or a rewind never steps; equal value sets present exactly as one.
/// `live_values` carries this frame's ledge focus in its pitch.
pub(super) fn present(
    weights: VaultWeights,
    live_values: PoseValues,
    state: &mut VaultRowState,
) -> (f32, PoseValues) {
    if !weights.hold {
        state.weight = weights.live.max(weights.release);
        state.values = blend(
            weights.release,
            state.release_values,
            weights.live,
            live_values,
        );
    }
    (state.weight, state.values)
}

/// The row's weight and blended eye drop this frame, before the line is
/// measured: eye drop never depends on focus, so its blend is known early.
pub(super) fn eye_drop(
    weights: VaultWeights,
    live_eye_drop: f32,
    state: &VaultRowState,
) -> (f32, f32) {
    if weights.hold {
        return (state.weight, state.values.eye_drop);
    }
    (
        weights.live.max(weights.release),
        blend_channel(
            weights.release,
            state.release_values.eye_drop,
            weights.live,
            live_eye_drop,
        ),
    )
}

fn blend(w_a: f32, a: PoseValues, w_b: f32, b: PoseValues) -> PoseValues {
    PoseValues {
        pitch: blend_channel(w_a, a.pitch, w_b, b.pitch),
        roll: blend_channel(w_a, a.roll, w_b, b.roll),
        eye_drop: blend_channel(w_a, a.eye_drop, w_b, b.eye_drop),
        fov: blend_channel(w_a, a.fov, w_b, b.fov),
    }
}

fn blend_channel(w_a: f32, a: f32, w_b: f32, b: f32) -> f32 {
    if w_a <= 0.0 {
        b
    } else if w_b <= 0.0 {
        a
    } else {
        (w_a * a + w_b * b) / (w_a + w_b)
    }
}

/// Ledge focus, degrees (≤ 0): `strength` of the gap from `reference_deg` down
/// to the line from `eye` to `landing`, capped at `max_pitch`. Nothing while
/// the reference is at or below the line, and nothing when the landing spot is
/// at or above the eye, so the pull is never upward. Pitch only: the bearing
/// to the landing spot never enters it.
pub(super) fn ledge_focus(focus: &LedgeFocus, eye: Vec3, landing: Vec3, reference_deg: f32) -> f32 {
    let rise = landing.y - eye.y;
    if rise.is_nan() || rise >= 0.0 {
        return 0.0;
    }
    let horizontal = Vec2::new(landing.x - eye.x, landing.z - eye.z)
        .length()
        .max(LEDGE_FOCUS_MIN_HORIZONTAL_M);
    let line_deg = rise.atan2(horizontal).to_degrees();
    let offset = (focus.strength * (line_deg - reference_deg).min(0.0)).max(-focus.max_pitch);
    if offset.is_finite() { offset } else { 0.0 }
}
