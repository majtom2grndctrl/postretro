// The NPC tick tiers: each NPC's tier, chosen on the host's fixed tick between
// the Projectiles and AI stages, and what that tier runs this tick. Engaged
// and idle NPCs act every tick and decide on their 15 Hz phase; a dormant NPC
// runs its whole tick at 2 Hz and decides on every fourth dormant run.
// Phases come from each NPC's identity, never spawn order or frame split.
// See: context/lib/entity_model.md §7c (decide ticks, tiers)

use std::collections::HashMap;

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, EntityTypeDescriptor, Transform,
};

use crate::agent_steering::AgentStep;
use crate::ai_host::AiRun;
use crate::npc_perception::{LevelCells, NpcPerception};

/// Ticks between an engaged or idle NPC's decides: 15 Hz on the 60 Hz tick.
pub const DECIDE_INTERVAL_TICKS: u64 = 4;
/// Ticks between a dormant NPC's runs: 2 Hz.
pub const DORMANT_INTERVAL_TICKS: u64 = 30;
/// A dormant NPC decides on every this-many dormant runs: every 2 s.
pub const DORMANT_RUNS_PER_DECIDE: u64 = 4;
/// How long an NPC with no perceiver, no target and no stimulus waits before
/// it moves down: 2 s, one dormant decide interval. Moving up never waits.
pub const MOVE_DOWN_HOLD_TICKS: u64 = DORMANT_INTERVAL_TICKS * DORMANT_RUNS_PER_DECIDE;

/// Whether an NPC with `identity` decides on tick `tick`, deciding every
/// `interval` ticks.
pub fn decides_on(tick: u64, identity: u64, interval: u64) -> bool {
    let interval = interval.max(1);
    (tick + phase(identity, interval)) % interval == 0
}

/// An identity's phase in a cycle of `interval` ticks: Fibonacci hashing,
/// the fractional part of `identity × φ` scaled to the cycle. Identities are
/// mostly counters (map ordinals, spawn counts), and a Weyl sequence spreads
/// any run of them, strided or interleaved, near evenly; a mixing hash would
/// spread them only as well as chance.
pub fn phase(identity: u64, interval: u64) -> u64 {
    let fraction = identity.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((u128::from(fraction) * u128::from(interval.max(1))) >> 64) as u64
}

/// Whether a dormant NPC runs on `tick`: its phase of the 2 Hz cycle.
pub fn dormant_runs_on(tick: u64, identity: u64) -> bool {
    decides_on(tick, identity, DORMANT_INTERVAL_TICKS)
}

/// Whether a dormant NPC's run on `tick` decides: every fourth of its dormant
/// runs, offset by a phase independent of its run phase.
pub fn dormant_decides_on(tick: u64, identity: u64) -> bool {
    if !dormant_runs_on(tick, identity) {
        return false;
    }
    let run = (tick + phase(identity, DORMANT_INTERVAL_TICKS)) / DORMANT_INTERVAL_TICKS;
    (run + phase(identity.swap_bytes(), DORMANT_RUNS_PER_DECIDE)) % DORMANT_RUNS_PER_DECIDE == 0
}

/// The identity an NPC's phases hash: its map placement or spawner and spawn
/// count, stored at seeding; else its registry index, replay-stable only while
/// spawn order is.
pub fn schedule_identity(brain: &BrainComponent, id: EntityId) -> u64 {
    brain
        .schedule_identity
        .unwrap_or_else(|| postretro_foundation::BehaviorDrawSalt::Entity(id.to_raw()).word())
}

/// An NPC's tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpcTier {
    /// Has a target, perceived or not.
    Engaged,
    /// Perceived, `alwaysActive`, stimulated, or inside the move-down hold;
    /// no target.
    Idle,
    /// Unperceived, no target, not `alwaysActive`, past the hold.
    Dormant,
}

#[derive(Debug, Clone, Copy)]
struct TierEntry {
    tier: NpcTier,
    /// The last tick it was perceived, `alwaysActive`, engaged or stimulated;
    /// the move-down hold counts from it. `None` before any.
    last_active: Option<u64>,
    /// The last tick steering stepped it.
    last_step: u64,
    /// Its aggro gate as the previous tier choice read it.
    aggro_armed: bool,
    /// The archetype's `alwaysActive`, cached against a descriptor generation.
    archetype_always_active: Option<(u64, bool)>,
    ai: AiRun,
    step: AgentStep,
    /// The tick that last chose for it, to prune departed NPCs.
    seen: u64,
}

/// Each NPC's tier and this tick's choice for it. Owned with the level; every
/// buffer is reused, so a tick allocates nothing once each NPC has an entry.
#[derive(Debug, Default)]
pub struct NpcTiers {
    tick: u64,
    entries: HashMap<EntityId, TierEntry>,
}

/// Inputs to one tier choice.
pub struct TierInputs<'a> {
    pub level: &'a dyn LevelCells,
    pub descriptors: &'a [EntityTypeDescriptor],
    pub descriptor_generation: u64,
}

impl NpcTiers {
    /// Choose every NPC's tier for this tick. Runs after the perception step,
    /// before the AI stage. A dead, inventory-owned or terminally removed NPC
    /// leaves the tiers; one returning rejoins by the usual rules.
    pub fn choose(
        &mut self,
        registry: &EntityRegistry,
        perception: &mut NpcPerception,
        inputs: &TierInputs<'_>,
    ) {
        self.tick += 1;
        let tick = self.tick;
        for (id, value) in registry.iter_with_kind(ComponentKind::Brain) {
            let ComponentValue::Brain(brain) = value else {
                continue;
            };
            if crate::sim::player_life::ai_is_quiescent(registry, id) {
                continue;
            }
            let Ok(transform) = registry.get_component::<Transform>(id) else {
                continue;
            };
            let perceived = perception.npc_perceived(inputs.level, id, transform.position);
            let fresh = !self.entries.contains_key(&id);
            let entry = self.entries.entry(id).or_insert(TierEntry {
                tier: NpcTier::Dormant,
                last_active: None,
                last_step: tick.saturating_sub(1),
                aggro_armed: brain.aggro_armed,
                archetype_always_active: None,
                ai: AiRun::Run,
                step: AgentStep::Ticks(1),
                seen: tick,
            });
            entry.seen = tick;

            let archetype = match entry.archetype_always_active {
                Some((generation, value)) if generation == inputs.descriptor_generation => value,
                _ => {
                    let value = archetype_always_active(registry, id, inputs.descriptors);
                    entry.archetype_always_active = Some((inputs.descriptor_generation, value));
                    value
                }
            };
            let always_active = brain.always_active.resolve(archetype);
            let engaged = brain.acquired_target.is_some();
            // A hit since its last run: the AI zeroes the clock at the hit and
            // ages it only when the brain runs.
            let damaged = brain.time_since_damage_ms == 0.0;
            let aggro_opened = brain.aggro_armed && !entry.aggro_armed;
            entry.aggro_armed = brain.aggro_armed;
            if perceived || always_active || engaged || damaged || aggro_opened {
                entry.last_active = Some(tick);
            }
            let held = entry
                .last_active
                .is_some_and(|last| tick - last < MOVE_DOWN_HOLD_TICKS);
            entry.tier = if engaged {
                NpcTier::Engaged
            } else if held {
                NpcTier::Idle
            } else {
                NpcTier::Dormant
            };

            let identity = schedule_identity(brain, id);
            (entry.ai, entry.step) = match entry.tier {
                // A move up runs at once: the AI credits the ticks it has not
                // run (its span), and steering moves one ordinary tick. An NPC
                // joining the tiers (spawned, revived) runs its first tick
                // whatever its tier, so its clocks start there.
                _ if fresh => (AiRun::Run, AgentStep::Ticks(1)),
                NpcTier::Engaged | NpcTier::Idle => (AiRun::Run, AgentStep::Ticks(1)),
                NpcTier::Dormant if dormant_runs_on(tick, identity) => (
                    AiRun::RunDormant {
                        decide: dormant_decides_on(tick, identity),
                    },
                    AgentStep::Ticks(u32::try_from(tick - entry.last_step).unwrap_or(u32::MAX)),
                ),
                NpcTier::Dormant => (AiRun::Skip, AgentStep::CarryOnly),
            };
            if matches!(entry.step, AgentStep::Ticks(_)) {
                entry.last_step = tick;
            }
        }
        // `retain` keeps the table's capacity.
        self.entries.retain(|_, entry| entry.seen == tick);
    }

    /// After the AI stage: a dormant NPC that acquired a target on its own run
    /// is engaged from now, and this tick's steering moves it one ordinary
    /// tick; its timers were credited by that run, never again.
    pub fn settle_after_ai(&mut self, registry: &EntityRegistry) {
        let tick = self.tick;
        for (id, entry) in &mut self.entries {
            if !matches!(entry.ai, AiRun::RunDormant { .. }) {
                continue;
            }
            let engaged = registry
                .get_component::<BrainComponent>(*id)
                .is_ok_and(|brain| brain.acquired_target.is_some());
            if engaged {
                entry.tier = NpcTier::Engaged;
                entry.last_active = Some(tick);
                entry.step = AgentStep::Ticks(1);
            }
        }
    }

    /// What the AI does for `id` this tick. NPCs outside the tiers run.
    pub fn ai_run(&self, id: EntityId) -> AiRun {
        self.entries.get(&id).map_or(AiRun::Run, |entry| entry.ai)
    }

    /// How steering steps `id` this tick. Agents outside the tiers step every
    /// tick.
    pub fn agent_step(&self, id: EntityId) -> AgentStep {
        self.entries
            .get(&id)
            .map_or(AgentStep::Ticks(1), |entry| entry.step)
    }

    /// `id`'s rank in the replan budget, higher first within each replan
    /// tier: engaged 2, idle 1, dormant 0. Agents outside the tiers rank as
    /// idle.
    pub fn replan_rank(&self, id: EntityId) -> u8 {
        match self.tier(id) {
            Some(NpcTier::Engaged) => 2,
            Some(NpcTier::Idle) | None => 1,
            Some(NpcTier::Dormant) => 0,
        }
    }

    /// `id`'s tier, `None` outside the tiers (dead, removed, or not an NPC).
    pub fn tier(&self, id: EntityId) -> Option<NpcTier> {
        self.entries.get(&id).map(|entry| entry.tier)
    }

    /// The tier choice's tick count.
    pub fn tick(&self) -> u64 {
        self.tick
    }
}

/// The archetype's live `alwaysActive`, found by the NPC's descriptor
/// provenance. `false` for an NPC with none.
fn archetype_always_active(
    registry: &EntityRegistry,
    id: EntityId,
    descriptors: &[EntityTypeDescriptor],
) -> bool {
    registry
        .get_component::<DescriptorProvenance>(id)
        .ok()
        .and_then(|provenance| {
            crate::scripting::builtins::data_archetype::find_descriptor(
                descriptors,
                &provenance.canonical_name,
            )
        })
        .and_then(|descriptor| descriptor.behavior.as_ref())
        .is_some_and(|behavior| behavior.ai.always_active)
}

/// The scheduler for the loaded level: perception and tiers.
#[derive(Debug)]
pub struct NpcScheduler {
    pub perception: NpcPerception,
    pub tiers: NpcTiers,
}

impl NpcScheduler {
    pub fn new(perception: NpcPerception) -> Self {
        Self {
            perception,
            tiers: NpcTiers::default(),
        }
    }

    pub fn for_level(level: &postretro_level_loader::LevelWorld) -> Self {
        Self::new(NpcPerception::for_level(level))
    }
}

/// The level-side input to one tick's scheduling: the loaded level's cells
/// and the scheduler built with it.
pub struct ScheduleInput<'a> {
    pub level: &'a dyn LevelCells,
    pub scheduler: &'a mut NpcScheduler,
}

#[cfg(test)]
#[path = "npc_tiers/tests.rs"]
mod tests;
