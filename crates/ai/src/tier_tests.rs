// The AI under the NPC tick tiers: the sim's tier choice picks who runs, and
// the AI credits a skipped brain's clocks once when it next runs. A pawn in
// arena 0 of a synthetic three-arena level perceives that arena; arena 2 is
// unseen.
// See: context/plans/in-progress/E10--ai-tick-scheduling/index.md (Tier
//      changes; P3, P4, P21)

use super::*;
use postretro_sim::agent_steering::AgentStep;
use postretro_sim::npc_perception::test_support::{Row, centre, spawn_at, three_arenas};
use postretro_sim::npc_perception::{GhostLinkCells, NpcPerception, resolve_ai_lead};
use postretro_sim::npc_tiers::{
    DECIDE_INTERVAL_TICKS, DORMANT_INTERVAL_TICKS, NpcScheduler, NpcTier, TierInputs,
};

const DT: f32 = 1.0 / 60.0;

struct Tiered {
    reg: EntityRegistry,
    runtime: AiRuntime,
    scheduler: NpcScheduler,
    level: Row,
    pawn: EntityId,
}

impl Tiered {
    fn new() -> Self {
        let level = three_arenas();
        let mut reg = EntityRegistry::new();
        let pawn = spawn_at(&mut reg, centre(&level, 3));
        let lead = resolve_ai_lead(None, level.residency.as_ref().map(|set| set.max_lead));
        Self {
            reg,
            runtime: AiRuntime::new(),
            scheduler: NpcScheduler::new(NpcPerception::new(30, lead, GhostLinkCells::default())),
            level,
            pawn,
        }
    }

    fn npc(&mut self, cell: usize, identity: u64, graph: &BehaviorGraphDescriptor) -> EntityId {
        let mut brain = BrainComponent::from_graph(graph);
        brain.schedule_identity = Some(identity);
        spawn_enemy(&mut self.reg, centre(&self.level, cell), brain, 100.0)
    }

    fn place(&mut self, id: EntityId, cell: usize) {
        let mut transform = *self.reg.get_component::<Transform>(id).unwrap();
        transform.position = centre(&self.level, cell);
        self.reg.set_component(id, transform).unwrap();
    }

    /// One tick: tier choice, the AI under it, then the post-AI settle.
    fn tick(&mut self) {
        let NpcScheduler { perception, tiers } = &mut self.scheduler;
        perception.step(&self.level, &self.reg, [self.pawn]);
        tiers.choose(
            &self.reg,
            perception,
            &TierInputs {
                level: &self.level,
                descriptors: &[],
                descriptor_generation: 0,
            },
        );
        let lookup = |id| tiers.ai_run(id);
        run_ai_tick_with_navigation_and_impact(
            &mut self.reg,
            &mut self.runtime,
            DT,
            AiTickInputs {
                mover_nav: crate::nav::MoverNav::none(),
                nav_graph: None,
                collision_world: Some(&CollisionWorld::new()),
                descriptors: &[],
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
                clips: None,
                schedule: crate::ai_host::AiSchedule::scheduled(&lookup),
                cpu: None,
            },
            |_| {},
        );
        tiers.settle_after_ai(&self.reg);
    }

    fn brain(&self, id: EntityId) -> &BrainComponent {
        self.reg.get_component::<BrainComponent>(id).unwrap()
    }

    fn tier(&self, id: EntityId) -> Option<NpcTier> {
        self.scheduler.tiers.tier(id)
    }
}

/// One hold activity and nothing else: its clock only ever advances.
fn resting_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "rest".to_string(),
        activities: BTreeMap::from([(
            "rest".to_string(),
            authored_state("idle", MotionVerb::Hold, None),
        )]),
        transitions: BTreeMap::new(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// `rest` → `hurt` once `@brain.timeSinceDamageMs` is at most 100.
fn hurt_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "rest".to_string(),
        activities: BTreeMap::from([
            ("rest".to_string(), authored_state("idle", MotionVerb::Hold, None)),
            ("hurt".to_string(), authored_state("idle", MotionVerb::Hold, None)),
        ]),
        transitions: BTreeMap::from([(
            "rest".to_string(),
            vec![edge(
                "hurt",
                IrNode::Le {
                    a: Box::new(brain_input(BRAIN_TIME_SINCE_DAMAGE_MS_INPUT)),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(100.0),
                    }),
                },
            )],
        )]),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

fn activity_clock(tiered: &Tiered, id: EntityId) -> f32 {
    tiered.brain(id).time_in_activity_ms[0]
}

// P3 / P11 / AC 45: an unseen NPC moved up mid-interval runs on that tick,
// its clocks credited by every tick it did not run, once; its steering moves
// one ordinary tick. Its 60 Hz twin, seen throughout, reads the same clock.
#[test]
fn move_up_credits_timers_once_and_steps_once() {
    let graph = resting_graph();
    for into_interval in [1, 7, 16, 29] {
        let mut tiered = Tiered::new();
        let seen = tiered.npc(5, 1, &graph);
        let unseen = tiered.npc(25, 2, &graph);
        // Spawn tick, then dormant runs and skips.
        for _ in 0..2 * DORMANT_INTERVAL_TICKS + into_interval {
            tiered.tick();
        }
        assert_eq!(tiered.tier(unseen), Some(NpcTier::Dormant));
        tiered.place(unseen, 6);
        tiered.tick();
        assert_eq!(tiered.tier(unseen), Some(NpcTier::Idle));
        assert_eq!(
            tiered.scheduler.tiers.agent_step(unseen),
            AgentStep::Ticks(1),
            "a move up never catches up movement"
        );
        let twin = activity_clock(&tiered, seen);
        assert!(
            (activity_clock(&tiered, unseen) - twin).abs() < 0.05,
            "{into_interval} ticks in: dormant {} ms, 60 Hz {twin} ms",
            activity_clock(&tiered, unseen)
        );
        for _ in 0..DORMANT_INTERVAL_TICKS {
            tiered.tick();
        }
        let twin = activity_clock(&tiered, seen);
        assert!(
            (activity_clock(&tiered, unseen) - twin).abs() < 0.05,
            "credited twice: dormant {} ms, 60 Hz {twin} ms",
            activity_clock(&tiered, unseen)
        );
    }
}

// P4: a dormant NPC decides on every fourth dormant run and on no other tick.
#[test]
fn a_dormant_npc_decides_on_every_fourth_dormant_run_only() {
    let graph = super::decide::toggling_graph();
    let mut tiered = Tiered::new();
    let npc = tiered.npc(25, 3, &graph);
    tiered.tick();
    let mut previous = tiered.brain(npc).active_activity_path;
    let mut decides = 0;
    for _ in 0..8 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
        let run = tiered.scheduler.tiers.ai_run(npc);
        let path = tiered.brain(npc).active_activity_path;
        let flipped = path != previous;
        previous = path;
        let deciding = run == crate::ai_host::AiRun::RunDormant { decide: true };
        assert_eq!(
            flipped, deciding,
            "a dormant decide flips; nothing else does"
        );
        decides += usize::from(deciding);
    }
    assert_eq!(decides, 2, "two decides in 8 dormant runs");
}

// AC 10: a dormant NPC hurt by contextless damage (a trap, a script) moves up
// and reads the hit in its guards at its next decide, never aged by the time
// it spent dormant before the hit.
#[test]
fn idle_npc_reads_contextless_damage_at_next_decide() {
    let graph = hurt_graph();
    for into_interval in [3, 17, 28] {
        let mut tiered = Tiered::new();
        let npc = tiered.npc(25, 4, &graph);
        for _ in 0..DORMANT_INTERVAL_TICKS + into_interval {
            tiered.tick();
        }
        assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
        // The damage chokepoint zeroes the clock, as a trap's hit does.
        let mut brain = tiered.brain(npc).clone();
        brain.time_since_damage_ms = 0.0;
        tiered.reg.set_component(npc, brain).unwrap();
        let mut decided_within = None;
        for tick in 1..=DECIDE_INTERVAL_TICKS + 1 {
            tiered.tick();
            if tiered.brain(npc).active_path_names() == ["hurt"] {
                decided_within = Some(tick);
                break;
            }
        }
        assert!(
            decided_within.is_some(),
            "{into_interval} ticks in: the hit was not read within a decide interval"
        );
        assert_eq!(tiered.tier(npc), Some(NpcTier::Idle));
    }
}

// P39 / AC 28: an unseen `alwaysActive` NPC runs exactly as its seen twin,
// every tick: the same AI run and steering step, the same decides, so a walk
// already under way crosses a trigger on the same tick as when perceived.
#[test]
fn unseen_always_active_npc_runs_every_tick_as_its_seen_twin() {
    let graph = super::decide::toggling_graph();
    let mut tiered = Tiered::new();
    let seen = tiered.npc(5, 9, &graph);
    let unseen = tiered.npc(25, 9, &graph);
    let mut brain = tiered.brain(unseen).clone();
    brain.always_active = postretro_entities::components::brain::AlwaysActive::On;
    tiered.reg.set_component(unseen, brain).unwrap();
    for _ in 0..4 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
        let tiers = &tiered.scheduler.tiers;
        assert_eq!(tiers.tier(unseen), Some(NpcTier::Idle));
        assert_eq!(tiers.ai_run(unseen), tiers.ai_run(seen));
        assert_eq!(tiers.agent_step(unseen), tiers.agent_step(seen));
        assert_eq!(
            tiered.brain(unseen).active_activity_path,
            tiered.brain(seen).active_activity_path
        );
        assert_eq!(
            tiered.brain(unseen).time_in_activity_ms,
            tiered.brain(seen).time_in_activity_ms
        );
    }
}
