//! Engine constants behind stances' defaults: what an attack gets when its
//! author leaves a value out, the co-op floor a stance's hold is measured
//! against, and the bound an impact flinch plays within. Stance lowering and
//! the AI's release exit both read them.
//! See: context/lib/entity_model.md §7c (Stances and attacks)

/// The default recovery duration, in milliseconds.
///
/// An attack with no authored `recover` holds its release's state until the
/// clip plays through or this much time passes, whichever comes first, so a
/// looping release still recovers; an authored `recover` without `ms` leaves
/// the same way. It also caps an impact flinch during a release or recover:
/// the reaction clip plays through, but never for longer than this, counted
/// from the first switch, before the enemy enters its ending stance.
///
/// A starting point, tuned on `movement-feel` with the shipped content.
pub const DEFAULT_RECOVERY_MS: f32 = 600.0;

/// The remote interpolation buffer's ceiling, in milliseconds: how late a
/// remote player can see an enemy's pose (netcode's `MAX_DELAY_MICROS`).
///
/// Netcode depends on this crate, not the reverse, so the ceiling is restated
/// here; a netcode test fails if [`STANCE_HOLD_FLOOR_MS`] falls below its own
/// clamp, as one does for the spawn windup.
pub const REMOTE_INTERPOLATION_CEILING_MS: f32 = 250.0;

/// Time a player needs to read a tell and react once it is on screen, in
/// milliseconds. A starting point, tuned on `movement-feel` under the lag
/// profile.
pub const STANCE_REACTION_BUDGET_MS: f32 = 150.0;

/// The co-op floor on a stance's hold, in milliseconds: a tell a remote
/// player cannot see is no tell. It covers the interpolation ceiling plus the
/// reaction budget, never network transit, which has no load-time bound.
///
/// Lowering reads it: an omitted `holdMs` starts at it, and an authored one
/// whose minimum falls below it loads with one warning per mod load naming
/// the stance. It is a default and a warning, not a guarantee.
pub const STANCE_HOLD_FLOOR_MS: f32 = REMOTE_INTERPOLATION_CEILING_MS + STANCE_REACTION_BUDGET_MS;

/// The hold range an omitted `holdMs` takes: `[min, max]` milliseconds from
/// the floor, wide enough that a player cannot learn the release.
pub const DEFAULT_HOLD_MS: [f32; 2] = [STANCE_HOLD_FLOOR_MS, STANCE_HOLD_FLOOR_MS + 600.0];
