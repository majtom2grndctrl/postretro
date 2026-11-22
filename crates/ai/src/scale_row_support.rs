// Shared scaffolding for the measurement rows (`replan_stress_row`,
// `multi_arena_row`): a deterministic LCG, median and worst helpers, the
// player's orbit, replan-wait bookkeeping, and the driver that runs one fixed
// tick either through the real NPC tiers over a synthetic cell level
// (perception, tier choice, AI under the schedule, settle, steering under the
// tiers' steps) or as today's 60 Hz NPC (every NPC decides and acts every
// tick). They prove nothing and only print.
// See: context/plans/done/E10--ai-tick-scheduling/index.md (Scale)
//      context/lib/testing_guide.md §Resource bounds

use std::cell::RefCell;
use std::time::{Duration, Instant};

use super::*;
use crate::agent_steering::{AgentSteps, AgentTickResult};
use crate::ai_host::{AiSchedule, AiStageFrame};
use crate::nav::arena_fixture::{ARENA, ARENAS, GAP, arena_x0};
use postretro_sim::npc_perception::test_support::{Row, residency};
use postretro_sim::npc_perception::{GhostLinkCells, NpcPerception, resolve_ai_lead};
use postretro_sim::npc_tiers::{NpcScheduler, NpcTier, TierInputs};

pub(super) const PLAYER_ORBIT_RADIUS: f32 = 2.0;
pub(super) const PLAYER_ORBIT_PERIOD_S: f32 = 4.0;
/// Mirrors `agent_steering`'s private `REPLAN_DEST_THRESHOLD`.
pub(super) const DRIFT_THRESHOLD: f32 = 0.5;
/// The fixed step, against which a worst tick is read.
pub(super) const FIXED_STEP_MS: f64 = 1_000.0 / 60.0;

/// Deterministic LCG in [0, 1).
pub(super) struct Lcg(pub(super) u64);

impl Lcg {
    pub(super) fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }
}

pub(super) fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

/// Median and worst of a run's per-tick times, in milliseconds.
#[derive(Clone, Copy, Default)]
pub(super) struct Spread {
    pub(super) p50: f64,
    pub(super) worst: f64,
}

impl Spread {
    pub(super) fn of(times: &[Duration]) -> Self {
        if times.is_empty() {
            return Self::default();
        }
        let mut sorted = times.to_vec();
        sorted.sort_unstable();
        Self {
            p50: ms(sorted[(sorted.len() - 1) / 2]),
            worst: ms(*sorted.last().unwrap()),
        }
    }
}

pub(super) fn percentile(sorted: &[Duration], p: f32) -> Duration {
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

/// The player's walk: a 2 m circle every 4 s around `center`.
pub(super) fn orbit(tick: usize, center: Vec3) -> (f32, f32) {
    let t = tick as f32 * STEER_DT;
    let angle = t / PLAYER_ORBIT_PERIOD_S * std::f32::consts::TAU;
    (
        center.x + angle.sin() * PLAYER_ORBIT_RADIUS,
        center.z + angle.cos() * PLAYER_ORBIT_RADIUS,
    )
}

/// An agent is owed a replan when its live destination has drifted past the
/// threshold from the planned one, or it has never planned.
pub(super) fn owed(agent: &AgentComponent) -> bool {
    let Some(destination) = agent.destination else {
        return false;
    };
    agent
        .planned_destination
        .is_none_or(|planned| distance_xz(planned, destination) > DRIFT_THRESHOLD)
}

/// Replan waits: the run of consecutive owed ticks that ends when steering
/// admits the agent.
#[derive(Default)]
pub(super) struct WaitStats {
    /// Completed waits, in ticks (0 = admitted on the tick it became owed).
    pub(super) waits: Vec<u32>,
    /// Waits still open when the run ended.
    pub(super) open: Vec<u32>,
    pub(super) admissions: u32,
}

impl WaitStats {
    pub(super) fn line(&mut self, label: &str, agents: usize) -> String {
        self.waits.sort_unstable();
        let pct = |p: f32| -> u32 {
            if self.waits.is_empty() {
                return 0;
            }
            let i = ((self.waits.len() - 1) as f32 * p).round() as usize;
            self.waits[i]
        };
        let mean = if self.waits.is_empty() {
            0.0
        } else {
            self.waits.iter().map(|&w| w as f64).sum::<f64>() / self.waits.len() as f64
        };
        let open_max = self.open.iter().copied().max().unwrap_or(0);
        format!(
            "{label:<17} agents {agents:>3} | admissions {:>5} | wait ticks mean {mean:>5.1} p50 {:>3} p95 {:>3} max {:>3} | open at end {:>3} (longest {open_max})",
            self.admissions,
            pct(0.5),
            pct(0.95),
            self.waits.last().copied().unwrap_or(0),
            self.open.len(),
        )
    }
}

/// Per-agent replan-wait tracking over a run: call [`Self::before_steering`]
/// before each sampled steering tick and [`Self::after_steering`] after it.
pub(super) struct WaitTracker {
    open: Vec<Option<u32>>,
    owed_now: Vec<bool>,
}

impl WaitTracker {
    pub(super) fn new(agents: usize) -> Self {
        Self {
            open: vec![None; agents],
            owed_now: vec![false; agents],
        }
    }

    pub(super) fn before_steering(&mut self, reg: &EntityRegistry, agents: &[EntityId]) {
        for (owed_now, &id) in self.owed_now.iter_mut().zip(agents) {
            // Steering skips the dead, so a corpse's last request is not a wait.
            *owed_now = reg.exists(id) && player_hp(reg, id) > 0.0 && owed(&enemy_agent(reg, id));
        }
    }

    /// Record admissions and waits into `stats`, one per agent.
    pub(super) fn after_steering(
        &mut self,
        reg: &EntityRegistry,
        agents: &[EntityId],
        mut stats: impl FnMut(usize) -> Option<usize>,
        all: &mut [WaitStats],
    ) {
        for (slot, &id) in agents.iter().enumerate() {
            if !reg.exists(id) {
                self.open[slot] = None;
                continue;
            }
            let agent = enemy_agent(reg, id);
            // Admission sets the cooldown to the full staleness window, after
            // this tick's decrement, so it reads exactly that value now.
            let admitted = agent.replan_cooldown_ticks == agent_steering::REPLAN_STALENESS_TICKS;
            let class = stats(slot);
            if admitted && let Some(class) = class {
                all[class].admissions += 1;
            }
            if self.owed_now[slot] {
                let wait = self.open[slot].unwrap_or(0);
                if admitted {
                    if let Some(class) = class {
                        all[class].waits.push(wait);
                    }
                    self.open[slot] = None;
                } else {
                    self.open[slot] = Some(wait + 1);
                }
            } else {
                self.open[slot] = None;
            }
        }
    }

    /// Fold the waits still open at the end into `all`.
    pub(super) fn finish(
        &self,
        mut stats: impl FnMut(usize) -> Option<usize>,
        all: &mut [WaitStats],
    ) {
        for (slot, wait) in self.open.iter().enumerate() {
            if let (Some(wait), Some(class)) = (wait, stats(slot)) {
                all[class].open.push(*wait);
            }
        }
    }
}

/// Synthetic id-51 levels for the rows, and the real NPC scheduler over one.
pub(super) struct TierLevel {
    pub(super) scheduler: NpcScheduler,
    pub(super) level: Row,
}

/// Cell width of the multi-arena level: 8 m, so an arena is 8 cells and the
/// corridor gap between two is 5.
const ARENA_CELL: f32 = 8.0;

impl TierLevel {
    fn new(level: Row) -> Self {
        let count = level
            .residency
            .as_ref()
            .map_or(1, |set| set.camera_cell_count());
        let lead = resolve_ai_lead(None, level.residency.as_ref().map(|set| set.max_lead));
        Self {
            scheduler: NpcScheduler::new(NpcPerception::new(
                count,
                lead,
                GhostLinkCells::default(),
            )),
            level,
        }
    }

    /// One cell that sees itself: the replan stress floor, all of it in view.
    pub(super) fn single_cell() -> Self {
        Self::new(Row {
            width: 10_000.0,
            residency: Some(residency(1, 32.0, |_| vec![(0, 0.0)])),
        })
    }

    /// The multi-arena map: one 8 m cell column per slice along x, each
    /// arena's cells seeing exactly that arena, each corridor cell itself.
    pub(super) fn multi_arena() -> Self {
        let per_arena = (ARENA / ARENA_CELL) as usize;
        let per_gap = (GAP / ARENA_CELL) as usize;
        let stride = per_arena + per_gap;
        let count = ARENAS * per_arena + (ARENAS - 1) * per_gap;
        Self::new(Row {
            width: ARENA_CELL,
            residency: Some(residency(count, 32.0, |camera| {
                let arena = camera / stride;
                if camera % stride < per_arena {
                    (arena * stride..arena * stride + per_arena)
                        .map(|cell| (cell, 0.0))
                        .collect()
                } else {
                    vec![(camera, 0.0)]
                }
            })),
        })
    }

    pub(super) fn tier(&self, id: EntityId) -> Option<NpcTier> {
        self.scheduler.tiers.tier(id)
    }

    /// (engaged, idle, dormant) over every NPC in the tiers.
    pub(super) fn counts(&self, reg: &EntityRegistry) -> [usize; 3] {
        let mut counts = [0; 3];
        for (id, _) in reg.iter_with_kind(postretro_entities::ComponentKind::Brain) {
            match self.tier(id) {
                Some(NpcTier::Engaged) => counts[0] += 1,
                Some(NpcTier::Idle) => counts[1] += 1,
                Some(NpcTier::Dormant) => counts[2] += 1,
                None => {}
            }
        }
        counts
    }
}

/// The arena an x position stands in, if any.
pub(super) fn arena_of(x: f32) -> Option<usize> {
    (0..ARENAS).find(|&arena| (arena_x0(arena)..arena_x0(arena) + ARENA).contains(&x))
}

/// Everything one tick needs besides the registry and runtime.
pub(super) struct World<'a> {
    pub(super) nav: &'a NavGraph,
    pub(super) collision: &'a CollisionWorld,
    pub(super) factions: &'a FactionRegistry,
    pub(super) sentiment: &'a RefCell<FactionSentimentState>,
    /// Closed doors the AI sees at their pose; none unless the run asks.
    pub(super) doors: &'a ArenaDoors,
}

/// One closed door per arena for the mover-sight scale rows
/// (`E17--movers-block-shots-and-sight`, S1): 8 m wide and 4 m tall, across
/// the arena's x axis at its centre, near its z = 8 edge. Present only when
/// `POSTRETRO_SCALE_DOORS=1`; otherwise empty, and every row runs as before.
pub(super) struct ArenaDoors {
    pub(super) movers: Vec<crate::collision::moving::MoverCollider>,
    pub(super) poses: crate::collision::test_movers::DoorPoses,
}

impl ArenaDoors {
    pub(super) fn from_env() -> Self {
        let wanted = std::env::var("POSTRETRO_SCALE_DOORS").is_ok_and(|value| value == "1");
        let mut doors = Self {
            movers: Vec::new(),
            poses: Default::default(),
        };
        if wanted {
            for arena in 0..ARENAS {
                let id = 1_000 + arena as u32;
                doors
                    .movers
                    .push(crate::collision::test_movers::door_slab(id, 0.1, 4.0, 4.0));
                doors.poses.set(
                    id,
                    Vec3::new(arena_x0(arena) + ARENA * 0.5, 0.0, 8.0),
                    glam::Quat::IDENTITY,
                );
            }
        }
        doors
    }
}

/// One tick's measured stages.
#[derive(Clone, Copy, Default)]
pub(super) struct TickCost {
    /// The perception step and tier choice; zero without tiers.
    pub(super) perception: Duration,
    pub(super) ai: Duration,
    pub(super) steering: Duration,
    pub(super) steer: AgentTickResult,
}

/// Run one fixed tick: through `tiers` (perception over `perceivers`, tier
/// choice, AI under the schedule, settle, steering under the tiers' steps),
/// or, with none, every NPC deciding and acting every tick.
pub(super) fn run_tick(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    mut tiers: Option<&mut TierLevel>,
    perceivers: &[EntityId],
    world: &World<'_>,
    cpu: Option<&AiStageFrame>,
) -> TickCost {
    let mut cost = ai_stage(reg, runtime, tiers.as_deref_mut(), perceivers, world, cpu);
    steering_stage(reg, tiers.as_deref(), world, &mut cost);
    cost
}

/// The tick up to steering: the perception step and tier choice (with
/// `tiers`), the AI under the schedule, and the post-AI settle.
pub(super) fn ai_stage(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tiers: Option<&mut TierLevel>,
    perceivers: &[EntityId],
    world: &World<'_>,
    cpu: Option<&AiStageFrame>,
) -> TickCost {
    let mut cost = TickCost::default();
    let inputs = |schedule| AiTickInputs {
        nav_graph: Some(world.nav),
        mover_nav: crate::nav::MoverNav {
            gate: crate::nav::NavGate::OPEN,
            movers: &world.doors.movers,
            poses: &world.doors.poses,
        },
        collision_world: Some(world.collision),
        descriptors: &[],
        descriptor_generation: 0,
        factions: world.factions,
        faction_sentiment: world.sentiment,
        clips: None,
        schedule,
        cpu,
    };
    match tiers {
        Some(tiers) => {
            let TierLevel { scheduler, level } = tiers;
            let NpcScheduler {
                perception,
                tiers: npc_tiers,
            } = scheduler;
            let start = Instant::now();
            perception.step(level, reg, perceivers.iter().copied());
            npc_tiers.choose(
                reg,
                perception,
                &TierInputs {
                    level,
                    descriptors: &[],
                    descriptor_generation: 0,
                },
            );
            cost.perception = start.elapsed();
            let start = Instant::now();
            {
                let lookup = |id| npc_tiers.ai_run(id);
                run_ai_tick_with_navigation_and_impact(
                    reg,
                    runtime,
                    STEER_DT,
                    inputs(AiSchedule::scheduled(&lookup)),
                    |_| {},
                );
            }
            npc_tiers.settle_after_ai(reg);
            cost.ai = start.elapsed();
        }
        None => {
            let start = Instant::now();
            run_ai_tick_with_navigation_and_impact(
                reg,
                runtime,
                STEER_DT,
                inputs(AiSchedule::EVERY_TICK),
                |_| {},
            );
            cost.ai = start.elapsed();
        }
    }
    cost
}

/// Steering, under the tiers' steps and replan ranks with `tiers`, else every
/// agent every tick.
pub(super) fn steering_stage(
    reg: &mut EntityRegistry,
    tiers: Option<&TierLevel>,
    world: &World<'_>,
    cost: &mut TickCost,
) {
    let environment = postretro_collision::EnvironmentView::level_only(STEER_GRAVITY);
    let start = Instant::now();
    cost.steer = match tiers {
        Some(tiers) => {
            let npc_tiers = &tiers.scheduler.tiers;
            let step = |id| npc_tiers.agent_step(id);
            let rank = |id| npc_tiers.replan_rank(id);
            agent_steering::tick_with_steps(
                reg,
                world.collision,
                Some(world.nav),
                AgentSteps::ranked(&step, &rank),
                environment,
                STEER_DT,
            )
        }
        None => agent_steering::tick(reg, world.collision, Some(world.nav), environment, STEER_DT),
    };
    cost.steering = start.elapsed();
}

/// The runtime a run measures: the tiers' production cadence, or today's
/// 60 Hz NPC, which decides every tick and takes the best candidate on sight.
pub(super) fn runtime_for(tiered: bool) -> AiRuntime {
    if tiered {
        AiRuntime::new()
    } else {
        AiRuntime::deciding_every_tick()
    }
}
