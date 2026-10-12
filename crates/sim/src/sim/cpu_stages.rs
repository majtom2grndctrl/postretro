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
    /// Inside `Steering`: `agent_steering::tick` — path replans, separation
    /// and every agent's collide-and-slide.
    SteerAgents,
    /// Inside `Steering`: player and brain animation locomotion plus
    /// presentation pose inputs.
    SteerPose,
    /// Inside `Steering`: auto-close timers and the mover blocking pass.
    SteerMovers,
    /// Inside `Tick`: per-body environment resolution (gravity, push, fluid)
    /// for every body that resolves this tick — host movement's local and
    /// remote pawns (inside `Movement`) and agent gravity (inside
    /// `SteerAgents`). It spans two siblings, so it sits under `Tick` and
    /// overlaps them: never add it to its siblings. Absent when no body
    /// resolved.
    Environment,
    /// Inside `Weapons`: hitscan pellets' fluid-crossing queries for splash
    /// facts. Absent in a tick that fired no hitscan.
    WeaponFluidCrossings,
    /// Player-event evaluation and in-tick fires, after the tick settles.
    /// Runs outside `Tick`, so it reports beside it.
    PlayerEvents,
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
        Self::SteerAgents,
        Self::SteerPose,
        Self::SteerMovers,
        Self::Environment,
        Self::WeaponFluidCrossings,
        Self::PlayerEvents,
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
            Self::SteerAgents => "sim_steer_agents",
            Self::SteerPose => "sim_steer_pose",
            Self::SteerMovers => "sim_steer_movers",
            Self::Environment => "sim_environment",
            Self::WeaponFluidCrossings => "sim_weapon_fluid_crossings",
            Self::PlayerEvents => "sim_player_events",
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::Tick | Self::PlayerEvents => None,
            Self::SteerAgents | Self::SteerPose | Self::SteerMovers => Some(Self::Steering),
            Self::WeaponFluidCrossings => Some(Self::Weapons),
            _ => Some(Self::Tick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_stages_are_registered_with_their_labels_and_parents() {
        assert!(SimStage::ALL.contains(&SimStage::Environment));
        assert!(SimStage::ALL.contains(&SimStage::WeaponFluidCrossings));
        assert_eq!(SimStage::Environment.label(), "sim_environment");
        assert_eq!(
            SimStage::WeaponFluidCrossings.label(),
            "sim_weapon_fluid_crossings"
        );
        // Player resolves run inside `Movement` and agent resolves inside
        // `SteerAgents`, so the one stage summing both nests under the tick.
        assert_eq!(SimStage::Environment.parent(), Some(SimStage::Tick));
        assert_eq!(
            SimStage::WeaponFluidCrossings.parent(),
            Some(SimStage::Weapons)
        );
    }

    #[test]
    fn index_matches_position_in_all() {
        for (position, &stage) in SimStage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), position, "{stage:?}");
        }
    }
}
