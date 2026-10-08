// Mouse-wheel normalization: line-scroll gestures and pixel-delta notch counts.
// See: context/lib/input.md §2

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::types::PhysicalInput;

const WHEEL_DIAGNOSTICS_ENV: &str = "POSTRETRO_WHEEL_DIAGNOSTICS";

/// Whether the opt-in raw wheel-event experiment is enabled for this process.
///
/// This is intentionally process-scoped: the environment is read once at
/// startup and normal input processing remains unchanged.
pub(crate) fn wheel_diagnostics_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        wheel_diagnostics_enabled_from(std::env::var(WHEEL_DIAGNOSTICS_ENV).ok().as_deref())
    })
}

pub(super) fn wheel_diagnostics_enabled_from(value: Option<&str>) -> bool {
    value == Some("1")
}

pub(super) const LINE_SCROLL_GESTURE_REPEAT: Duration = Duration::from_millis(128);

/// Turns continuous line-scroll input into discrete selection steps. The first
/// event in a gesture steps immediately; sustained same-direction input repeats
/// at a fixed cadence rather than using the platform's accelerated magnitude.
#[derive(Debug, Default)]
pub(super) struct LineScrollGesture {
    direction_up: Option<bool>,
    last_event_at: Option<Instant>,
    last_step_at: Option<Instant>,
}

impl LineScrollGesture {
    pub(super) fn accepts(&mut self, delta: f64, now: Instant) -> bool {
        if !delta.is_finite() || delta == 0.0 {
            return false;
        }

        let direction_up = delta.is_sign_positive();
        let continues = self.direction_up == Some(direction_up)
            && self
                .last_event_at
                .is_some_and(|last| now.duration_since(last) < LINE_SCROLL_GESTURE_REPEAT);
        self.direction_up = Some(direction_up);
        self.last_event_at = Some(now);

        if !continues
            || self
                .last_step_at
                .is_none_or(|last| now.duration_since(last) >= LINE_SCROLL_GESTURE_REPEAT)
        {
            self.last_step_at = Some(now);
            return true;
        }
        false
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Explicit per-frame scroll count. Pixel remainders survive until they form a
/// whole notch; only the emitted counts reset each snapshot.
#[derive(Debug, Default)]
pub(super) struct ScrollNotchAccumulator {
    pub(super) up: u32,
    pub(super) down: u32,
    pub(super) pixel_remainder: f64,
}

impl ScrollNotchAccumulator {
    pub(super) fn add_pixel_delta(&mut self, delta: f64, pixels_per_notch: f64) {
        if !delta.is_finite() || !pixels_per_notch.is_finite() || pixels_per_notch <= 0.0 {
            return;
        }
        self.pixel_remainder += delta;
        let notches = (self.pixel_remainder / pixels_per_notch).trunc() as i64;
        self.pixel_remainder -= notches as f64 * pixels_per_notch;
        self.add_signed_notches(notches);
    }

    pub(super) fn add_signed_notches(&mut self, notches: i64) {
        if notches > 0 {
            self.up = self
                .up
                .saturating_add(u32::try_from(notches).unwrap_or(u32::MAX));
        } else if notches < 0 {
            self.down = self
                .down
                .saturating_add(u32::try_from(notches.unsigned_abs()).unwrap_or(u32::MAX));
        }
    }

    pub(super) fn count(&self, input: PhysicalInput) -> u32 {
        match input {
            PhysicalInput::MouseWheelUp => self.up,
            PhysicalInput::MouseWheelDown => self.down,
            _ => 0,
        }
    }

    pub(super) fn clear_frame(&mut self) {
        self.up = 0;
        self.down = 0;
    }

    pub(super) fn clear_all(&mut self) {
        self.clear_frame();
        self.pixel_remainder = 0.0;
    }
}
