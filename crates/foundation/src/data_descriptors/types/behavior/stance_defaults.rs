//! Engine constants behind stances' defaults: what an attack gets when its
//! author leaves a value out, and the bound an impact flinch plays within.
//! Stance lowering and the AI's release exit both read them.
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
