// The fixed tick's CPU stage set, reported per tick on `TickEvents::cpu`.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::StageSet;

/// Stages inside one `simulate_tick_with_presentation_aim` call. The binary
/// sums them across a frame's ticks and places `sim_tick` under its
/// fixed-step stage. Work the binary does around the call (post-tick host
/// bookkeeping) is fixed-step time outside `sim_tick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimStage {
    /// The whole authoritative tick.
    Tick,
    /// Kinematic mover advance and terminus events.
    Movers,
    /// Remote and local player movement.
    Movement,
    /// Trigger volumes (with their in-tick script dispatch) and touch pickups.
    Triggers,
    /// Ready remote-hit ingest and projectile flight.
    Projectiles,
    /// Enemy AI policy through the injected runner.
    Ai,
    /// Agent steering, animation locomotion, presentation poses, auto-close
    /// timers and the mover blocking pass.
    Steering,
    /// Remote and local weapon commands and projectile launch bookkeeping.
    Weapons,
}

impl StageSet for SimStage {
    const ALL: &'static [Self] = &[
        Self::Tick,
        Self::Movers,
        Self::Movement,
        Self::Triggers,
        Self::Projectiles,
        Self::Ai,
        Self::Steering,
        Self::Weapons,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Tick => "sim_tick",
            Self::Movers => "sim_movers",
            Self::Movement => "sim_movement",
            Self::Triggers => "sim_triggers",
            Self::Projectiles => "sim_projectiles",
            Self::Ai => "sim_ai",
            Self::Steering => "sim_steering",
            Self::Weapons => "sim_weapons",
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::Tick => None,
            _ => Some(Self::Tick),
        }
    }
}
