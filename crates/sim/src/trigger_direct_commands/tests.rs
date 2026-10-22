// A trigger's no-script `command` on Enter, over every brainless member kind,
// through the real trigger tick; and the install warning for unaccepted tags.
// See: context/lib/build_pipeline.md §Custom FGD (trigger_volume)

use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::projectile_trap::ProjectileTrapComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverMode,
    Transform, TriggerActivation, TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::traps::install::resolve_traps_for_level;
use crate::traps::run_trap_stage;
use crate::traps::trap_test_fixtures::{bolt, entity};
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};

const DT: f32 = 1.0 / 60.0;
const SPAWN_COUNT: u32 = 2;

/// One host level: a player who steps onto or off every trigger at the origin,
/// tagged members, and the production trigger tick.
struct Level {
    registry: EntityRegistry,
    bridge: TriggerVolumeBridge,
    system: TriggerSystem,
    spawn_context: SpawnContext,
    player: EntityId,
}

impl Level {
    fn new() -> Self {
        let mut registry = EntityRegistry::new();
        let player = registry.spawn(Transform {
            position: Vec3::new(50.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                player,
                crate::trigger_bindings::group_tick_tests::movement(),
            )
            .unwrap();
        let spawn_context = SpawnContext::default();
        spawn_context.replace_level_data(
            [("cultist".to_string(), behavior_enemy_descriptor("cultist"))]
                .into_iter()
                .collect(),
            None,
        );
        Self {
            registry,
            bridge: TriggerVolumeBridge::new(),
            system: TriggerSystem::default().with_spawn_context(spawn_context.clone()),
            spawn_context,
            player,
        }
    }

    fn trigger(&mut self, tag: &str, command: TriggerCommand) -> EntityId {
        self.trigger_with(tag, command, TriggerFireMode::Multiple, 0.0)
    }

    fn trigger_with(
        &mut self,
        tag: &str,
        command: TriggerCommand,
        fire_mode: TriggerFireMode,
        rearm_ms: f32,
    ) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    tag.into(),
                    String::new(),
                    String::new(),
                    command,
                    fire_mode,
                    rearm_ms,
                    true,
                ),
            )
            .unwrap();
        self.bridge
            .insert_for_test(id, Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
        id
    }

    fn tagged(&mut self, tag: &str) -> EntityId {
        self.registry
            .try_spawn(Transform::default(), &[tag.to_string()])
            .unwrap()
    }

    fn mover(&mut self, tag: &str) -> EntityId {
        let id = self.tagged(tag);
        self.registry
            .set_component(
                id,
                KinematicMoverComponent::new(
                    1,
                    postretro_entities::KinematicMoverConfig {
                        waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                        waypoint_names: vec!["start".into(), "end".into()],
                        speed_mps: 1.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::Once,
                        started: false,
                        spin_axis: Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        id
    }

    fn trap(&mut self, tag: &str) -> EntityId {
        let id = self.tagged(tag);
        self.registry
            .set_component(
                id,
                ProjectileTrapComponent::new("bolt".into(), 1000.0, 0.0, false),
            )
            .unwrap();
        resolve_traps_for_level(&mut self.registry, &[entity("bolt", bolt(100.0))], true);
        id
    }

    fn spawner(&mut self, tag: &str) -> EntityId {
        let id = self.tagged(tag);
        self.registry
            .set_component(
                id,
                SpawnerComponent {
                    archetype_name: "cultist".into(),
                    count: SPAWN_COUNT,
                    spawned_tags: vec!["wave".into()],
                    resolved: true,
                },
            )
            .unwrap();
        id
    }

    fn brain(&mut self, id: EntityId) {
        let graph = behavior_enemy_descriptor("cultist")
            .behavior
            .expect("the enemy fixture has a brain");
        self.registry
            .set_component(id, BrainComponent::from_graph(&graph))
            .unwrap();
    }

    fn place(&mut self, at: Vec3) {
        let mut transform = *self
            .registry
            .get_component::<Transform>(self.player)
            .unwrap();
        transform.position = at;
        self.registry.set_component(self.player, transform).unwrap();
    }

    /// One authoritative tick: triggers, then the trap stage. Returns the
    /// traps that launched a shot.
    fn tick(&mut self) -> Vec<EntityId> {
        let players = [AuthoritativePlayer {
            id: PlayerId::Local(self.player),
            pawn: self.player,
        }];
        self.system.run_authoritative_tick(
            &mut self.registry,
            &self.bridge,
            &players,
            &Default::default(),
            DT,
        );
        let output = run_trap_stage(&mut self.registry, DT);
        output
            .emissions
            .iter()
            .filter_map(|emission| match emission.emitter {
                postretro_entities::Emitter::Entity { id, .. } => Some(id),
                _ => None,
            })
            .collect()
    }

    fn enter(&mut self) -> Vec<EntityId> {
        self.place(Vec3::ZERO);
        self.tick()
    }

    fn leave(&mut self) -> Vec<EntityId> {
        self.place(Vec3::new(50.0, 0.0, 0.0));
        self.tick()
    }

    fn mover_started(&self, mover: EntityId) -> bool {
        let mover = self
            .registry
            .get_component::<KinematicMoverComponent>(mover)
            .unwrap();
        mover.started
    }

    fn trap_running(&self, trap: EntityId) -> bool {
        self.registry
            .get_component::<ProjectileTrapComponent>(trap)
            .unwrap()
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.schedule.is_some())
    }

    fn spawned(&self) -> usize {
        self.registry
            .query_by_component_and_tag(ComponentKind::Brain, Some("wave"))
            .count()
    }
}

#[test]
fn enter_fires_every_trap_carrying_the_tag_on_that_tick() {
    let mut level = Level::new();
    let traps = [
        level.trap("volley"),
        level.trap("volley"),
        level.trap("volley"),
    ];
    let other = level.trap("elsewhere");
    level.trigger("volley", TriggerCommand::Fire);
    let mut fired = level.enter();
    fired.sort_unstable();
    assert_eq!(fired, traps.to_vec());
    assert!(!fired.contains(&other));
}

#[test]
fn direct_start_and_stop_reach_movers_and_traps() {
    let mut level = Level::new();
    let mover = level.mover("pair");
    let trap = level.trap("pair");
    let start = level.trigger("pair", TriggerCommand::Start);
    level.enter();
    assert!(level.mover_started(mover));
    assert!(level.trap_running(trap));
    level.leave();
    level
        .registry
        .set_component(
            start,
            TriggerVolumeComponent {
                command: TriggerCommand::Stop,
                ..level
                    .registry
                    .get_component::<TriggerVolumeComponent>(start)
                    .unwrap()
                    .clone()
            },
        )
        .unwrap();
    level.enter();
    assert!(!level.mover_started(mover));
    assert!(!level.trap_running(trap));
}

// P12: only accepting members act, and one accepting member keeps it quiet.
#[test]
fn direct_commands_reach_only_accepting_kinds() {
    let capture = LogCapture::start();
    let mut level = Level::new();
    let mover = level.mover("mixed");
    let trap = level.trap("mixed");
    level.trigger("mixed", TriggerCommand::Fire);
    assert_eq!(level.enter(), vec![trap], "fire fires the trap");
    assert!(!level.mover_started(mover), "and leaves the mover still");

    let mut level = Level::new();
    let mover = level.mover("mixed");
    let trap = level.trap("mixed");
    level.trigger("mixed", TriggerCommand::Reverse);
    assert!(level.enter().is_empty(), "reverse leaves the trap silent");
    let reversed = level
        .registry
        .get_component::<KinematicMoverComponent>(mover)
        .unwrap()
        .clone();
    assert!(reversed.started, "reverse reverses the mover");
    assert!(!level.trap_running(trap));

    let mut level = Level::new();
    let trap = level.trap("closet");
    level.spawner("closet");
    level.trigger("closet", TriggerCommand::Fire);
    assert_eq!(level.enter(), vec![trap]);
    assert_eq!(level.spawned(), SPAWN_COUNT as usize, "one batch");
    warn_unaccepted_trigger_commands(&level.registry);
    capture.assert_not_logged(log::Level::Warn, "reaches no member");
}

#[test]
fn direct_fire_spawns_one_batch_per_spawner_on_the_host_only() {
    let mut level = Level::new();
    level.spawner("closet");
    level.spawner("closet");
    level.trigger("closet", TriggerCommand::Fire);
    level.enter();
    assert_eq!(level.spawned(), 2 * SPAWN_COUNT as usize);

    let mut client = Level::new();
    client.spawn_context.set_runtime_spawn_authority(false);
    client.spawner("closet");
    client.trigger("closet", TriggerCommand::Fire);
    client.enter();
    assert_eq!(client.spawned(), 0, "spawn authority gates the batch");
}

#[test]
fn direct_commands_skip_brained_members() {
    let mut level = Level::new();
    let trap = level.trap("guarded");
    let mover = level.mover("guarded");
    level.brain(trap);
    level.brain(mover);
    level.trigger("guarded", TriggerCommand::Start);
    assert!(level.enter().is_empty());
    assert!(!level.trap_running(trap), "a brained trap is skipped");
    assert!(!level.mover_started(mover), "a brained mover is skipped");

    // An enemy a spawner released carries the spawner's `spawned_tags`.
    let mut level = Level::new();
    level.spawner("closet");
    level.trigger("closet", TriggerCommand::Fire);
    level.trigger_with("wave", TriggerCommand::Fire, TriggerFireMode::Multiple, 0.0);
    level.enter();
    let enemies: Vec<_> = level
        .registry
        .query_by_component_and_tag(ComponentKind::Brain, Some("wave"))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(enemies.len(), SPAWN_COUNT as usize);
    let before: Vec<_> = enemies
        .iter()
        .map(|&id| *level.registry.get_component::<Transform>(id).unwrap())
        .collect();
    level.leave();
    level.enter();
    for (id, transform) in enemies.iter().zip(before) {
        assert_eq!(
            *level.registry.get_component::<Transform>(*id).unwrap(),
            transform,
            "a spawned enemy is untouched by a trigger on its tag"
        );
    }
}

#[test]
fn install_warns_once_for_a_tag_no_member_accepts() {
    let mut level = Level::new();
    level.mover("lift");
    level.trigger("lift", TriggerCommand::Fire);
    level.trigger("", TriggerCommand::Fire);
    level.trigger("nobody", TriggerCommand::Fire);
    level.trigger("lift", TriggerCommand::Start);
    let capture = LogCapture::start();
    warn_unaccepted_trigger_commands(&level.registry);
    capture.assert_logged_once(
        log::Level::Warn,
        "command `fire` reaches no member tagged `lift`",
    );
    capture.assert_logged_once(log::Level::Warn, "reaches no member");
}

// Regression guard: a direct mover `start` still starts a brainless mover.
#[test]
fn direct_start_still_starts_a_tagged_mover() {
    let mut level = Level::new();
    let mover = level.mover("lift");
    level.trigger_with("lift", TriggerCommand::Start, TriggerFireMode::Once, 0.0);
    level.enter();
    assert!(level.mover_started(mover));
}

// Regression guard: a `multiple` trigger refires its traps only after re-arm.
#[test]
fn rearmed_trigger_refires_traps_only_after_rearm() {
    let mut level = Level::new();
    let trap = level.trap("volley");
    level.trigger_with(
        "volley",
        TriggerCommand::Fire,
        TriggerFireMode::Multiple,
        500.0,
    );
    assert_eq!(level.enter(), vec![trap]);
    let rearm_ticks = (500.0 / (DT * 1000.0)).ceil() as usize;
    let mut refired = None;
    for tick in 1..rearm_ticks + 60 {
        let fired = if tick % 2 == 1 {
            level.leave()
        } else {
            level.enter()
        };
        if !fired.is_empty() {
            refired = Some(tick);
            break;
        }
    }
    let refired = refired.expect("the trap fires again after re-arm");
    assert!(
        refired >= rearm_ticks - 1,
        "refired at tick {refired}, before the {rearm_ticks}-tick re-arm"
    );
}
