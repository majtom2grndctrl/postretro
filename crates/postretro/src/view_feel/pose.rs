// Sustained camera pose held while a movement state lasts (`viewFeel.pose`):
// one row per state, each on its own timing, their outputs added.
// See: context/lib/movement.md §1

mod vault_row;

#[cfg(test)]
mod tests;

use glam::Vec3;
use postretro_foundation::{MovementStateKind, PoseChannels, PoseParams, PoseRow};

pub(crate) use vault_row::VaultViewInput;

/// Every row's eye drop together must leave this much clearance above the
/// pawn's feet.
pub(crate) const MIN_EYE_CLEARANCE: f32 = 0.05;

/// One row's channels: degrees, and metres for `eye_drop`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct PoseValues {
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
    pub(crate) eye_drop: f32,
    pub(crate) fov: f32,
}

impl From<PoseChannels> for PoseValues {
    fn from(channels: PoseChannels) -> Self {
        Self {
            pitch: channels.pitch,
            roll: channels.roll,
            eye_drop: channels.eye_drop,
            fov: channels.fov,
        }
    }
}

impl PoseValues {
    fn scaled(self, weight: f32) -> Self {
        Self {
            pitch: self.pitch * weight,
            roll: self.roll * weight,
            eye_drop: self.eye_drop * weight,
            fov: self.fov * weight,
        }
    }

    fn plus(self, other: Self) -> Self {
        Self {
            pitch: self.pitch + other.pitch,
            roll: self.roll + other.roll,
            eye_drop: self.eye_drop + other.eye_drop,
            fov: self.fov + other.fov,
        }
    }
}

/// Render-side integrator for every row. Not tick state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct PoseState {
    /// The slide row's weight, before the view-feel scale.
    slide_weight: f32,
    vault: vault_row::VaultRowState,
}

/// The movement facts and eye one frame's pose reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PoseFrame {
    pub(crate) movement_state: MovementStateKind,
    /// The presented vault, while the pawn vaults.
    pub(crate) vault: Option<VaultViewInput>,
    /// The aim pitch this frame renders, degrees, after this frame's look.
    pub(crate) aim_pitch_deg: f32,
    /// The rendered eye before any row's drop: bob and the other positional
    /// offsets included.
    pub(crate) eye: Vec3,
    /// How far every row's drop together may lower that eye before it meets
    /// the [`MIN_EYE_CLEARANCE`] floor.
    pub(crate) available_drop: f32,
}

/// Every row's output summed, scaled by the view-feel scale. Angles in degrees;
/// `eye_drop` in metres, already floored.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct PoseOutput {
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
    pub(crate) eye_drop: f32,
    pub(crate) fov: f32,
}

/// Advance every row by one frame and sum their outputs.
///
/// Rows evaluate independently, so a handoff eases one row out while the next
/// eases in. Their eye drops sum under one floor, and the vault row's ledge
/// line is measured from the eye after it. Weights advance under any scale, so
/// restoring the view-feel scale resumes from the live blend; a zero-length
/// frame holds every row's weight and values.
pub(crate) fn evaluate(
    params: Option<&PoseParams>,
    frame: PoseFrame,
    state: &mut PoseState,
    frame_dt: f32,
    scale: f32,
) -> PoseOutput {
    let Some(params) = params else {
        *state = PoseState::default();
        return PoseOutput::default();
    };
    let slide = match params.slide {
        Some(row) => {
            advance_rate_row(
                &row,
                frame.movement_state == MovementStateKind::Slide,
                &mut state.slide_weight,
                frame_dt,
            );
            PoseValues::from(row.channels).scaled(state.slide_weight * scale)
        }
        None => {
            state.slide_weight = 0.0;
            PoseValues::default()
        }
    };
    let vault = params.vault.map(|row| {
        let weights = vault_row::advance(
            &row,
            frame.vault,
            frame.aim_pitch_deg,
            &mut state.vault,
            frame_dt,
        );
        (row, weights)
    });
    if vault.is_none() {
        state.vault = vault_row::VaultRowState::default();
    }

    let vault_drop = vault.map_or(0.0, |(row, weights)| {
        let (weight, eye_drop) = vault_row::eye_drop(weights, row.channels.eye_drop, &state.vault);
        eye_drop * (weight * scale)
    });
    // The floor bounds the sum, not each row: two rows easing through a
    // handoff never put the eye under it.
    let eye_drop = (slide.eye_drop + vault_drop).min(frame.available_drop);

    let vault = vault.map_or(PoseValues::default(), |(row, weights)| {
        let mut live = PoseValues::from(row.channels);
        if let (Some(focus), Some(landing)) =
            (row.ledge_focus, frame.vault.and_then(|input| input.landing))
        {
            let eye = frame.eye - Vec3::Y * eye_drop;
            live.pitch += vault_row::ledge_focus(
                &focus,
                eye,
                landing,
                vault_row::reference_deg(&state.vault),
            );
        }
        let (weight, values) = vault_row::present(weights, live, &mut state.vault);
        values.scaled(weight * scale)
    });

    let total = slide.plus(vault);
    PoseOutput {
        pitch: total.pitch,
        roll: total.roll,
        eye_drop,
        fov: total.fov,
    }
}

/// Ease a rate row's weight toward 1 while its state is current and toward 0
/// otherwise, so an exit with no edge (a reconcile writing `Normal`) still
/// eases out.
fn advance_rate_row(row: &PoseRow, current: bool, weight: &mut f32, frame_dt: f32) {
    let (target, rate) = if current {
        (1.0, row.enter_rate)
    } else {
        (0.0, row.exit_rate)
    };
    let alpha = 1.0 - (-rate * frame_dt.max(0.0)).exp();
    *weight += (target - *weight) * alpha;
}
