// Sustained camera pose held while a movement state lasts: `viewFeel.pose`.
// See: context/lib/movement.md §1 (view feel)

use serde::{Deserialize, Serialize};

/// State-keyed sustained camera poses. Each row is independently optional and
/// evaluates on its own timing; their outputs add. `normal` has no row: a pose
/// there would be a permanent offset, not a state cue. Presentation only: aim,
/// collision and the wire are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PoseParams {
    /// Held while sliding, eased by rates.
    pub slide: Option<PoseRow>,
    /// Held while vaulting, weighted by the climb's progress.
    pub vault: Option<VaultPoseRow>,
}

/// The four literal channels every row holds. A missing channel is 0.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PoseChannels {
    /// Degrees, positive looks up. Finite in `[-MAX_ANGLE, MAX_ANGLE]`.
    pub pitch: f32,
    /// Degrees. Finite in `[-MAX_ANGLE, MAX_ANGLE]`.
    pub roll: f32,
    /// Metres downward, finite and ≥ 0. Every row's drop sums under one
    /// eye-to-feet floor.
    pub eye_drop: f32,
    /// Horizontal degrees added to the base FOV. Finite in `[-MAX_FOV, MAX_FOV]`.
    pub fov: f32,
}

impl PoseChannels {
    pub const MAX_ANGLE: f32 = 90.0;
    pub const MAX_FOV: f32 = 90.0;
}

/// A row timed by rates: its weight eases toward 1 at `enter_rate` while its
/// state is current and toward 0 at `exit_rate` otherwise, so an exit with no
/// edge still eases out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PoseRow {
    pub channels: PoseChannels,
    /// 1/sec, finite in `[MIN_RATE, MAX_RATE]`.
    pub enter_rate: f32,
    /// 1/sec, finite in `[MIN_RATE, MAX_RATE]`.
    pub exit_rate: f32,
}

impl PoseRow {
    pub const MIN_RATE: f32 = 0.1;
    pub const MAX_RATE: f32 = 240.0;
}

/// The vault row, weighted by the climb's own progress: it eases in over the
/// first `ease_in` share, holds, and eases out over the last `ease_out` share,
/// reaching neutral as the pawn reaches the top.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VaultPoseRow {
    pub channels: PoseChannels,
    /// Share of the climb, in `[0, 1]`; default [`Self::DEFAULT_EASE_IN`].
    pub ease_in: f32,
    /// Share of the climb, in `[0, 1]`; default [`Self::DEFAULT_EASE_OUT`].
    /// `ease_in + ease_out` is at most 1.
    pub ease_out: f32,
    pub ledge_focus: Option<LedgeFocus>,
}

impl VaultPoseRow {
    pub const DEFAULT_EASE_IN: f32 = 0.15;
    pub const DEFAULT_EASE_OUT: f32 = 0.2;
}

/// A downward pitch offset toward the line from the rendered eye to the
/// vault's landing spot, measured against the aim the pose took at the vault's
/// entry: `strength` of the gap while that reference is above the line, capped
/// at `max_pitch` degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LedgeFocus {
    /// Share of the gap, in `[0, 1]`.
    pub strength: f32,
    /// Degrees, in `(0, MAX_PITCH]`.
    pub max_pitch: f32,
}

impl LedgeFocus {
    pub const MAX_PITCH: f32 = 89.0;
}
