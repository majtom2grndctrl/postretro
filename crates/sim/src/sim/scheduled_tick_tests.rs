// The NPC tiers through the scheduled fixed tick's real stage order: an unseen
// NPC's steering steps only on its dormant phase, and a hit moves it up on the
// tick it lands when that is before the AI stage, otherwise on the next.
// See: context/lib/entity_model.md §7c

use postretro_entities::Transform;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_foundation::DamagePayload;

use super::tests::{sim_command, trigger_movement};
use super::*;
use crate::agent_steering::AgentStep;
use crate::ai_host::AiRun;
use crate::npc_perception::test_support::{Row, centre, three_arenas};
use crate::npc_perception::{GhostLinkCells, NpcPerception, resolve_ai_lead};
use crate::npc_tiers::{NpcScheduler, NpcTier, ScheduleInput, dormant_runs_on};

const TICK_DT: f32 = 1.0 / 60.0;

/// A listen host on the three-arena synthetic level, its pawn in arena 0, with
/// the level's NPC scheduler and a production-cadence AI.
struct Host {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    row: Row,
    scheduler: NpcScheduler,
    ai: postretro_ai::AiRuntime,
    movers: MoverTickStateTable,
    touch: TouchSystem,
}

impl Host {
    fn new() -> Self {
        let row = three_arenas();
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform {
                position: centre(&row, 3),
                ..Transform::default()
            });
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
        }
        let perception = NpcPerception::new(
            30,
            resolve_ai_lead(None, row.residency.as_ref().map(|set| set.max_lead)),
            GhostLinkCells::default(),
        );
        Self {
            registry,
            world: CollisionWorld::new(),
            row,
            scheduler: NpcScheduler::new(perception),
            ai: postretro_ai::AiRuntime::new(),
            movers: MoverTickStateTable::default(),
            touch: TouchSystem::default(),
        }
    }

    /// A brained, damageable agent at `cell` whose identity puts its dormant
    /// runs on tier tick `runs_on` (and every 30 ticks from it).
    fn npc(&mut self, cell: usize, runs_on: u64) -> EntityId {
        let identity = (0..)
            .find(|&identity| dormant_runs_on(runs_on, identity))
            .expect("some identity takes every phase");
        let graph =
            crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor(
                "scheduled_grunt",
            )
            .behavior
            .unwrap();
        let mut brain = BrainComponent::from_graph(&graph);
        brain.schedule_identity = Some(identity);
        let position = centre(&self.row, cell);
        let mut registry = self.registry.borrow_mut();
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry.set_component(id, brain).unwrap();
        registry
            .set_component(id, AgentComponent::new(0.35, 1.8, 0.4, 3.0))
            .unwrap();
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        id
    }

    /// One scheduled fixed tick. `before_ai` takes a hit in the Projectiles
    /// stage, as a remote hit ready at the input boundary does; `after_ai` takes
    /// one after the AI stage, as an NPC's attack or a trap shot does.
    fn tick(&mut self, before_ai: Option<EntityId>, after_ai: Option<EntityId>) {
        let Self {
            registry,
            world,
            row,
            scheduler,
            ai,
            movers,
            touch,
        } = self;
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let edges = HashMap::new();
        let _ = simulate_tick_scheduled(
            registry.clone(),
            world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            postretro_ai::test_tick_runner!(&mut *ai),
            Some(ScheduleInput {
                level: &*row,
                scheduler,
            }),
            &[],
            movers,
            &[],
            &sim_command(false, false),
            |registry| {
                if let Some(npc) = after_ai {
                    hit(&mut registry.borrow_mut(), npc);
                }
                PostMovementCommand {
                    aim_origin: Vec3::ZERO,
                    aim_direction: Vec3::NEG_Z,
                }
            },
            TICK_DT,
            touch,
            &mut Default::default(),
            &[],
            0,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            None,
            &edges,
            &edges,
            None,
            |registry, _| {
                if let Some(npc) = before_ai {
                    hit(registry, npc);
                }
            },
            |_| {},
            TimingGate::OFF,
        );
    }

    fn height(&self, id: EntityId) -> f32 {
        self.registry
            .borrow()
            .get_component::<Transform>(id)
            .unwrap()
            .position
            .y
    }
}

/// A contextless 1-point hit through the damage chokepoint.
fn hit(registry: &mut EntityRegistry, npc: EntityId) {
    let landed = apply_damage_with_context(
        registry,
        npc,
        &DamagePayload {
            amount: 1.0,
            impulse: Vec3::ZERO,
            damage_type: None,
        },
        DamageContext::new("test.scheduled_hit", DamageProducer::InTick),
    );
    assert!(landed, "the hit landed");
}

// The test runner hands the AI no schedule (its `AiRun` is the other sim build's
// type), so every brain runs each tick; what this proves is the sim side: the
// tier choice between Projectiles and AI, and steering stepping by it.
#[test]
fn scheduled_tick_moves_a_hit_dormant_npc_up_by_the_stage_it_lands_in() {
    let mut host = Host::new();
    // Both unseen in arena 2: `early` runs dormant on tier tick 5, `late` not
    // before tick 20.
    let early = host.npc(25, 5);
    let late = host.npc(27, 20);

    // Each joins with one ordinary run, then steps only on its dormant phase.
    // Nothing holds them up in this empty world, so a step is a fall.
    for tick in 1..=7_u64 {
        let before = [host.height(early), host.height(late)];
        host.tick(None, None);
        assert_eq!(host.scheduler.tiers.tick(), tick);
        for (npc, before) in [early, late].into_iter().zip(before) {
            assert_eq!(
                host.scheduler.tiers.tier(npc),
                Some(NpcTier::Dormant),
                "tick {tick}"
            );
            let step = host.scheduler.tiers.agent_step(npc);
            let expected = match tick {
                1 => AgentStep::Ticks(1),
                5 if npc == early => AgentStep::Ticks(4),
                _ => AgentStep::CarryOnly,
            };
            assert_eq!(step, expected, "tick {tick}");
            let moved = (host.height(npc) - before).abs() > 1.0e-6;
            assert_eq!(
                moved,
                matches!(step, AgentStep::Ticks(_)),
                "tick {tick}: steering moved {npc:?} only on its steps"
            );
        }
    }

    // Tick 8: `early` is hit in the Projectiles stage, `late` after the AI.
    let before = host.height(early);
    host.tick(Some(early), Some(late));
    assert_eq!(
        host.scheduler.tiers.tier(early),
        Some(NpcTier::Idle),
        "a hit before the AI stage moves its NPC up on that tick"
    );
    assert_eq!(host.scheduler.tiers.ai_run(early), AiRun::Run);
    assert_eq!(host.scheduler.tiers.agent_step(early), AgentStep::Ticks(1));
    assert!(
        (host.height(early) - before).abs() > 1.0e-6,
        "and steering stepped it"
    );
    assert_eq!(
        host.scheduler.tiers.tier(late),
        Some(NpcTier::Dormant),
        "a hit after the AI stage waits for the next tier choice"
    );
    assert_eq!(host.scheduler.tiers.agent_step(late), AgentStep::CarryOnly);

    host.tick(None, None);
    assert_eq!(
        host.scheduler.tiers.tier(late),
        Some(NpcTier::Idle),
        "moved up on the next tick"
    );
    assert_eq!(host.scheduler.tiers.ai_run(late), AiRun::Run);
    assert_eq!(host.scheduler.tiers.agent_step(late), AgentStep::Ticks(1));
    assert_eq!(
        host.scheduler.tiers.tier(early),
        Some(NpcTier::Idle),
        "held up through the move-down hold"
    );
}
