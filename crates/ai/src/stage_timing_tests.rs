// AI sub-stages through the production sim tick: targeting, positioning,
// perception, graph evaluation and apply report under `sim_ai`, steering's
// under `sim_steering`, and nothing reports with timing off.
// See: context/lib/rendering_pipeline.md §12 (CPU Stage Timing)

use super::*;
use crate::sim::cpu_stages::SimStage;
use crate::sim::{TickEvents, TimingGate};
use postretro_stage_timing::StageSet;

const AI_SUB_STAGES: [SimStage; 5] = [
    SimStage::AiTargeting,
    SimStage::AiPositioning,
    SimStage::AiPerception,
    SimStage::AiGraph,
    SimStage::AiApply,
];
const STEERING_SUB_STAGES: [SimStage; 3] = [
    SimStage::SteerAgents,
    SimStage::SteerPose,
    SimStage::SteerMovers,
];

struct Scene {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    nav: NavGraph,
    runtime: AiRuntime,
}

impl Scene {
    /// A player and, when `with_enemy`, one chaser 6 m from it on the open
    /// floor.
    fn new(with_enemy: bool) -> Self {
        Self::with(if with_enemy { Some(true) } else { None })
    }

    /// A player and one enemy 30 m off, past its detection range, resting
    /// unengaged.
    fn idle() -> Self {
        Self::with(Some(false))
    }

    /// `Some(true)` adds an engaged chaser, `Some(false)` an idle enemy.
    fn with(enemy: Option<bool>) -> Self {
        let with_enemy = enemy == Some(true);
        let floor = OpenFloor::new();
        let mut registry = EntityRegistry::new();
        spawn_player(&mut registry, target_pos());
        if enemy == Some(false) {
            spawn_enemy(
                &mut registry,
                target_pos() + Vec3::new(30.0, 0.0, 0.0),
                brain_with(test_graph_with(18.0, 18.0), TEST_IDLE_STATE),
                50.0,
            );
        }
        if with_enemy {
            let graph = engaged_graph(
                "engage",
                vec![("engage", activity("walk", MotionVerb::ChaseTarget, None))],
            );
            spawn_engaged(
                &mut registry,
                target_pos() + Vec3::new(6.0, 0.0, 0.0),
                &graph,
                "engage",
            );
        }
        Self {
            registry: Rc::new(RefCell::new(registry)),
            world: floor.collision_world(),
            nav: floor.nav_graph(),
            runtime: AiRuntime::new(),
        }
    }

    fn tick(&mut self, timing: TimingGate) -> TickEvents {
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut mover_states = MoverTickStateTable::default();
        let mut touch_system = TouchSystem::default();
        let no_edges = HashMap::new();
        crate::sim::simulate_tick_with_presentation_aim(
            self.registry.clone(),
            &self.world,
            &hit_zones,
            Some(&self.nav),
            postretro_collision::EnvironmentView::level_only(0.0),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            crate::tick_runner!(&mut self.runtime),
            &[],
            &mut mover_states,
            &[],
            &SimCommand {
                assist_held: false,
                input_tick: 0,
                secondary_button: postretro_sim::weapon::FireButtonState {
                    pressed: false,
                    active: false,
                },
                activation: postretro_foundation::ActivationInput::default(),
                movement: MovementInput {
                    wish_dir: Vec2::ZERO,
                    jump_pressed: false,
                    dash_pressed: false,
                    running: false,
                    crouch_intent: false,
                    facing_yaw: 0.0,
                    facing_pitch: 0.0,
                    use_pressed: false,
                    drop_pressed: false,
                },
                fire_button: crate::weapon::FireButtonState {
                    pressed: false,
                    active: false,
                },
                reload: false,
                firing_slot: 0,
                select_slot: None,
                use_pressed: false,
                drop_pressed: false,
            },
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            STEER_DT,
            &mut touch_system,
            &mut Default::default(),
            &[],
            0,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            None,
            &no_edges,
            &no_edges,
            None,
            |_, _| {},
            |_| {},
            timing,
        )
    }
}

fn value(events: &TickEvents, stage: SimStage) -> u64 {
    events
        .cpu
        .value(stage)
        .unwrap_or_else(|| panic!("{stage:?} reported"))
}

#[test]
fn ai_sub_stages_report_under_sim_ai_with_an_engaged_enemy() {
    let mut scene = Scene::new(true);
    let events = scene.tick(TimingGate::ON);

    let ai = value(&events, SimStage::Ai);
    let mut children = 0;
    for stage in AI_SUB_STAGES {
        assert_eq!(stage.parent(), Some(SimStage::Ai), "{stage:?}");
        let sub = value(&events, stage);
        assert!(sub <= ai, "{stage:?} {sub} ns exceeds sim_ai {ai} ns");
        children += sub;
    }
    // Siblings never overlap, so together they fit in the parent too.
    assert!(
        children <= ai,
        "sub-stages sum to {children} ns, over sim_ai {ai} ns"
    );
    let steering = value(&events, SimStage::Steering);
    for stage in STEERING_SUB_STAGES {
        assert_eq!(stage.parent(), Some(SimStage::Steering), "{stage:?}");
        let sub = value(&events, stage);
        assert!(
            sub <= steering,
            "{stage:?} {sub} ns exceeds sim_steering {steering} ns"
        );
    }
}

// Regression guard: positioning already returns before building any batch
// when no enemy is engaged, so its stage must not open there.
#[test]
fn ai_positioning_stage_is_absent_without_an_engaged_enemy() {
    for (label, mut scene) in [
        ("no enemy", Scene::new(false)),
        ("an idle enemy", Scene::idle()),
    ] {
        let events = scene.tick(TimingGate::ON);
        assert!(
            events.cpu.value(SimStage::Ai).is_some(),
            "{label}: the AI stage ran"
        );
        assert_eq!(events.cpu.value(SimStage::AiPositioning), None, "{label}");
    }
    let mut idle = Scene::idle();
    let events = idle.tick(TimingGate::ON);
    assert!(
        events.cpu.value(SimStage::AiGraph).is_some(),
        "the idle enemy was evaluated, so positioning's own early return held"
    );
}

#[test]
fn ai_sub_stages_are_absent_with_timing_off() {
    let mut scene = Scene::new(true);
    let events = scene.tick(TimingGate::OFF);

    for stage in AI_SUB_STAGES.into_iter().chain([SimStage::Ai]) {
        assert_eq!(events.cpu.value(stage), None, "{stage:?}");
    }
}
