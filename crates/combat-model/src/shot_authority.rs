// Shot-authorization types shared by simulation and netcode.
// See: context/lib/networking.md

use glam::Vec3;
use postretro_entities::EntityId;
use postretro_foundation::{KnockbackDescriptor, SplashDescriptor};
use postretro_net::wire::NetworkId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShotId(u64);

impl ShotId {
    pub fn from_parts(pawn: NetworkId, client_tick: u32) -> Self {
        Self((u64::from(pawn.0) << 32) | u64::from(client_tick))
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn client_tick(self) -> u32 {
        self.0 as u32
    }
}

pub const HIT_RANGE_TOLERANCE: f32 = 1.25;
pub const MAX_OPEN_SHOT_AGE_TICKS: u32 = 180;
/// Two seconds comfortably covers the conditioned co-op link's RTT and leaves
/// room for a delayed rendered-frame declaration after projectile travel.
pub const PROJECTILE_RTT_MARGIN_TICKS: u32 = 120;

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizedShot {
    pub shot_id: ShotId,
    pub pawn: EntityId,
    pub weapon: EntityId,
    pub fire_tick: u32,
    pub damage: f32,
    pub range: f32,
    pub pellet_count: usize,
    pub credit_source: String,
    /// Direct impulse tuning captured at FIRE; never taken from client hit data.
    pub knockback: Option<KnockbackDescriptor>,
    /// Immutable projectile-impact tuning captured at FIRE. Host hit intake
    /// must not reread a weapon that may have changed or despawned in flight.
    pub splash: Option<SplashDescriptor>,
    /// Collision radius captured with the projectile launch. It is host-only
    /// authority data used to reproduce the local swept-contact geometry.
    pub projectile_radius: Option<f32>,
    /// Frozen projectile flight facts. Splash intake replays this ray only as
    /// far as host time, range, and lifetime permit; the client's point never
    /// selects the detonation position.
    pub projectile_direction: Option<Vec3>,
    pub projectile_speed: Option<f32>,
    pub projectile_lifetime_seconds: Option<f32>,
    pub projectile_tick_seconds: Option<f32>,
    /// Frozen at FIRE because the weapon may be switched or despawned before a
    /// later projectile declaration arrives. This authority data never crosses
    /// the wire.
    pub is_projectile: bool,
    pub fire_origin: Vec3,
    pub timeout_budget_ticks: u32,
}

/// Keep a projectile declaration open through its maximum authored travel time
/// plus a deliberately generous return-trip margin. The `u32` serial tick clock
/// is wrap-aware only through half its range, so cap an absurd authored lifetime
/// there rather than accidentally retaining an open shot forever at wrap.
pub fn projectile_timeout_budget_ticks(
    range: f32,
    speed: f32,
    lifetime_seconds: f32,
    tick_dt_seconds: f32,
) -> u32 {
    let travel_seconds = if range.is_finite()
        && range >= 0.0
        && speed.is_finite()
        && speed > 0.0
        && lifetime_seconds.is_finite()
        && lifetime_seconds >= 0.0
        && tick_dt_seconds.is_finite()
        && tick_dt_seconds > 0.0
    {
        (f64::from(range) / f64::from(speed)).min(f64::from(lifetime_seconds))
    } else {
        return MAX_OPEN_SHOT_AGE_TICKS;
    };
    let max_budget = u32::MAX / 2;
    // Promote before dividing. Finite f32 authoring bounds can overflow either
    // division in f32 even though the corresponding duration is representable
    // well enough to saturate this bounded host-side retention budget.
    let travel_ticks = (travel_seconds / f64::from(tick_dt_seconds)).ceil();
    let travel_ticks = travel_ticks
        .min(f64::from(max_budget - PROJECTILE_RTT_MARGIN_TICKS))
        .max(0.0) as u32;
    MAX_OPEN_SHOT_AGE_TICKS.max(travel_ticks.saturating_add(PROJECTILE_RTT_MARGIN_TICKS))
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAuthorizedShot {
    pub shot: AuthorizedShot,
    pub owner_client_id: u64,
}
