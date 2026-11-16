// Host-steered enemy stride rate against a loopback client: the host runs the
// real fixed tick (AI, steering, playback rate) while snapshots carry only the
// pose and animation state, and the client derives the rate from the shared
// descriptor. An activity's `speed` must never split the two.
// See: context/lib/networking.md · context/lib/entity_model.md §7c

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec2;
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::mesh::{MeshAnimation, RATE_MAX};
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavRegion};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

use super::*;
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::movement::MovementInput;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::{PostMovementCommand, SimCommand, simulate_tick};
use crate::weapon::FireButtonState;
use postretro_sim::nav::NavGraph;

const PACED_CLASS: &str = "paced";
const MOVE_SPEED: f32 = 3.0;
const TICK_DT: f32 = 1.0 / 60.0;
const ENEMY_START: Vec3 = Vec3::new(4.0, 1.21, 32.0);
const PLAYER_POSITION: Vec3 = Vec3::new(62.0, 1.21, 32.0);
/// Host and client rates agree to within this; both sample one cruising pace.
const RATE_TOLERANCE: f32 = 0.02;

/// The paced enemy: one chasing root activity at `speed`, playing `state`,
/// optionally carrying an attack, as a moving release does.
fn paced_graph(state: &str, speed: f32, attacks: bool) -> BehaviorGraphDescriptor {
    let mut graph = enemy_behavior_graph();
    graph.move_speed = MOVE_SPEED;
    graph.envelope = BehaviorGraphEnvelope {
        initial: "pace".to_string(),
        activities: std::collections::BTreeMap::from([(
            "pace".to_string(),
            BehaviorActivityDescriptor {
                pose: None,
                stagger_to: None,
                sound: None,
                animation: Some(state.to_string()),
                motion: Some(MotionVerb::ChaseTarget),
                action: attacks.then(|| ActionVerb::Attack {
                    name: "attack".to_string(),
                    at: None,
                    at_default: false,
                }),
                on_enter: None,
                facing: None,
                reposition: None,
                attack_turn: None,
                speed: Some(speed),
                layers: Default::default(),
            },
        )]),
        transitions: Default::default(),
    };
    graph
}

fn paced_states(state: &str, travel_speed: Option<f32>) -> HashMap<String, AnimationState> {
    HashMap::from([
        (
            "idle".to_string(),
            AnimationState {
                clip: "idle_clip".to_string(),
                looping: true,
                crossfade_ms: 0.0,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        ),
        (
            state.to_string(),
            AnimationState {
                clip: format!("{state}_clip"),
                looping: true,
                crossfade_ms: 0.0,
                interrupt: InterruptPolicy::Smooth,
                travel_speed,
                clip_index: None,
            },
        ),
    ])
}

/// The descriptor both peers load: the same graph and mesh states the host
/// entity is built from.
fn paced_descriptor(
    graph: &BehaviorGraphDescriptor,
    states: &HashMap<String, AnimationState>,
) -> EntityTypeDescriptor {
    let mut descriptor = enemy_descriptor(PACED_CLASS);
    let mesh = descriptor.mesh.as_mut().expect("fixture enemy has a mesh");
    mesh.model = "paced_mesh".to_string();
    mesh.animations = states.clone();
    descriptor.behavior = Some(graph.clone());
    descriptor
}

fn floor_world() -> CollisionWorld {
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-500.0, 0.0, -500.0),
            Vec3::new(500.0, 0.0, -500.0),
            Vec3::new(500.0, 0.0, 500.0),
            Vec3::new(-500.0, 0.0, 500.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    )
}

fn open_floor_nav_graph() -> NavGraph {
    NavGraph::from_section(&NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: 64,
        dim_z: 64,
        agent_radius: 0.35,
        agent_height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions: vec![NavRegion {
            x0: 0,
            z0: 0,
            x1: 64,
            z1: 64,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        }],
        portals: vec![],
        gates: Vec::new(),
        links: Vec::new(),
    })
}

fn spawn_host_player(registry: &mut EntityRegistry) {
    let player = registry.spawn(Transform {
        position: PLAYER_POSITION,
        ..Transform::default()
    });
    registry
        .set_component(
            player,
            PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
                sounds: None,
                knockback: Default::default(),
                capsule: CapsuleParams {
                    radius: 0.4,
                    half_height: 0.8,
                    eye_height: 0.5,
                },
                ground: GroundParams {
                    speed: SpeedParams {
                        walk: 7.0,
                        run: 11.0,
                        crouch: 3.0,
                    },
                    accel: 10.0,
                    step_height: 0.3,
                    max_slope: 45.0,
                    friction: GroundParams::DEFAULT_FRICTION,
                    stop_speed: GroundParams::DEFAULT_STOP_SPEED,
                },
                air: AirParams {
                    forward_steer: 0.0,
                    accel: 0.7,
                    max_control_speed: 0.5,
                    bunny_hop: false,
                    jumps: 0,
                    jump_velocity: 5.5,
                    jump_ceiling: 0.0,
                },
                fall: FallParams {
                    terminal_velocity: 40.0,
                },
                stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
                stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
                dash: None,
                forgiveness: None,
                crouch: None,
                slide: None,
                swim: None,
                vault: None,
                view_feel: None,
            }),
        )
        .expect("host player receives movement state");
    registry
        .set_component(
            player,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .expect("host player receives health");
}

/// A map-placed host enemy built from the paced descriptor's own parts.
fn spawn_host_paced_enemy(
    registry: &mut EntityRegistry,
    graph: &BehaviorGraphDescriptor,
    states: &HashMap<String, AnimationState>,
    state: &str,
) -> EntityId {
    let enemy = registry.spawn(Transform {
        position: ENEMY_START,
        ..Transform::default()
    });
    let mut brain = BrainComponent::from_graph(graph);
    brain.home_anchor = ENEMY_START;
    registry.set_component(enemy, brain).unwrap();
    registry
        .entity_state_mut(enemy)
        .expect("enemy carries entity state")
        .set(
            postretro_foundation::FACTION_STATE_FIELD,
            postretro_entities::DEFAULT_ENEMY_FACTION_INDEX,
        );
    registry
        .set_component(enemy, AgentComponent::new(0.35, 1.8, 0.4, graph.move_speed))
        .unwrap();
    registry
        .set_component(
            enemy,
            MeshComponent::animated(
                "paced_mesh".to_string(),
                MeshAnimation::new(states.clone(), state.to_string()),
            ),
        )
        .unwrap();
    registry
        .set_component(
            enemy,
            ai_enemy_provenance(PACED_CLASS, DescriptorSpawnPath::MapPlacement),
        )
        .unwrap();
    enemy
}

/// The host's real fixed tick over the harness's host registry.
struct HostTick {
    world: CollisionWorld,
    nav_graph: NavGraph,
    hit_zones: HitZoneStore,
    progress: ProgressTracker,
    ai: postretro_ai::AiRuntime,
    movers: MoverTickStateTable,
    ticks: u32,
}

impl HostTick {
    fn new() -> Self {
        Self {
            world: floor_world(),
            nav_graph: open_floor_nav_graph(),
            hit_zones: HitZoneStore::new(),
            progress: ProgressTracker::new(),
            ai: postretro_ai::AiRuntime::new(),
            movers: MoverTickStateTable::default(),
            ticks: 0,
        }
    }

    fn run(&mut self, registry: &mut EntityRegistry) {
        self.ticks += 1;
        let shared = Rc::new(RefCell::new(std::mem::take(registry)));
        let _ = simulate_tick(
            shared.clone(),
            &self.world,
            &self.hit_zones,
            Some(&self.nav_graph),
            postretro_collision::EnvironmentView::level_only(-20.0),
            None,
            f64::from(self.ticks) * f64::from(TICK_DT),
            &mut self.progress,
            postretro_ai::tick_runner!(&mut self.ai),
            &[],
            &mut self.movers,
            &[],
            &neutral_command(),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::Z,
            },
            TICK_DT,
            None,
            |_| {},
        );
        *registry = Rc::try_unwrap(shared)
            .unwrap_or_else(|_| panic!("the tick keeps no registry handle"))
            .into_inner();
    }
}

fn neutral_command() -> SimCommand {
    SimCommand {
        assist_held: false,
        input_tick: 0,
        secondary_button: FireButtonState {
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
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

impl EnemyReplicationHarness {
    /// [`Self::client_receive`] through the production descriptor-aware
    /// materialization, which also caches each remote enemy's walk playback
    /// reference from the shared descriptor.
    fn client_receive_with_walk_playback(&mut self) {
        let hit_zones = HitZoneStore::new();
        let mut acks = Vec::new();
        for packet in self.to_client.take_ready() {
            let Ok(raw) = wire::decode::<RawSnapshotMessage>(&packet) else {
                continue;
            };
            let Ok(snapshot) = raw.validate() else {
                continue;
            };
            let outcome = self
                .client_replication
                .apply_snapshot(&mut self.client_registry, &snapshot);
            crate::snapshot_apply::materialize_remote_entities(
                &outcome.remote_entities,
                &self.descriptors,
                &mut self.client_registry,
                &mut self.client_replication,
                snapshot.server_tick,
                Some(agent_params()),
                &hit_zones,
            );
            acks.extend(outcome.ack);
        }
        for ack in &acks {
            self.server_replication.apply_ack(
                CLIENT_ID,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
    }
}

/// What one host-and-client run presents once the enemy cruises.
struct PacedRun {
    host_speed: f32,
    host_rate: f32,
    client_rate: f32,
}

/// Run the paced enemy on the host's fixed tick, replicate each tick over a
/// perfect loopback link, and sample the client inside its buffered window.
fn paced_run(state: &str, speed: f32, travel_speed: Option<f32>, attacks: bool) -> PacedRun {
    let graph = paced_graph(state, speed, attacks);
    let states = paced_states(state, travel_speed);
    let mut h = EnemyReplicationHarness::new(perfect_link());
    h.descriptors = vec![paced_descriptor(&graph, &states)];
    spawn_host_player(&mut h.host_registry);
    let enemy = spawn_host_paced_enemy(&mut h.host_registry, &graph, &states, state);
    h.host_register_descriptors();
    let net_id = h.enemy_network_id(enemy);
    let mut host = HostTick::new();
    for _ in 0..60 {
        host.run(&mut h.host_registry);
        h.host_tick();
        h.client_receive_with_walk_playback();
    }

    let host_speed = {
        let velocity = h
            .host_registry
            .get_component::<AgentComponent>(enemy)
            .expect("host enemy keeps its agent")
            .steer_velocity;
        Vec3::new(velocity.x, 0.0, velocity.z).length()
    };
    let host_rate = h
        .host_registry
        .get_component::<MeshComponent>(enemy)
        .expect("host enemy keeps its mesh")
        .animation
        .as_ref()
        .expect("host mesh is animated")
        .rate;

    let remote = h
        .client_mapped_entity(net_id)
        .expect("the client maps the paced enemy");
    let render_tick = f64::from(h.server_tick.saturating_sub(4));
    h.client_replication
        .sample_into_registry(&mut h.client_registry, render_tick, 1.0);
    let client_animation = h
        .client_registry
        .get_component::<MeshComponent>(remote)
        .expect("the remote enemy carries its presentation mesh")
        .animation
        .clone()
        .expect("the remote mesh is animated");
    assert_eq!(
        client_animation.current_state, state,
        "the client presents the host's state"
    );
    PacedRun {
        host_speed,
        host_rate,
        client_rate: client_animation.rate,
    }
}

fn assert_rates_match(run: &PacedRun, expected: f32, label: &str) {
    assert!(
        (run.host_rate - expected).abs() <= RATE_TOLERANCE,
        "{label}: host rate {} (expected {expected})",
        run.host_rate
    );
    assert!(
        (run.client_rate - run.host_rate).abs() <= RATE_TOLERANCE,
        "{label}: client rate {} vs host {}",
        run.client_rate,
        run.host_rate
    );
}

// AC Sp5: an activity with `speed: 2` playing the graph's locomotion state
// presents the same rate on host and loopback client, with and without a
// calibrated stride. Both divide by the archetype's base `moveSpeed` when
// uncalibrated, so a host that divided by the steered speed would rest at 1.
// A `speed` the rate clamp cannot hide (1.25) pins the uncalibrated reference.
#[test]
fn a_sped_locomotion_activity_presents_one_rate_on_host_and_loopback_client() {
    let uncalibrated = paced_run("locomotion", 2.0, None, false);
    assert!(
        (uncalibrated.host_speed - 2.0 * MOVE_SPEED).abs() <= 0.05,
        "the host steers at twice moveSpeed: {}",
        uncalibrated.host_speed
    );
    assert_rates_match(
        &uncalibrated,
        RATE_MAX,
        "uncalibrated speed 2: twice moveSpeed clamps",
    );

    let unclamped = paced_run("locomotion", 1.25, None, false);
    assert_rates_match(
        &unclamped,
        1.25,
        "uncalibrated speed 1.25: steered over base moveSpeed",
    );

    let stride = 1.6 * MOVE_SPEED;
    let calibrated = paced_run("locomotion", 2.0, Some(stride), false);
    assert_rates_match(
        &calibrated,
        2.0 * MOVE_SPEED / stride,
        "calibrated speed 2: steered over the state's travelSpeed",
    );
}

// AC Sp3 (engine half): a moving activity that also attacks — a charge
// release — rate-scales its own state against that state's `travelSpeed` on
// host and loopback client, so its stride matches its speed. Before the shared
// rate-scaled rule such a state rested at rate 1 on both.
#[test]
fn a_charging_attack_activity_strides_against_its_own_travel_speed_on_host_and_client() {
    let stride = 1.6 * MOVE_SPEED;
    let charge = paced_run("run", 2.0, Some(stride), true);
    assert!(
        (charge.host_speed - 2.0 * MOVE_SPEED).abs() <= 0.05,
        "the charge steers at twice moveSpeed: {}",
        charge.host_speed
    );
    assert_rates_match(
        &charge,
        2.0 * MOVE_SPEED / stride,
        "charge on `run`: steered speed over its travelSpeed",
    );
}

/// A stance's rest and hold sharing one pose and one animation state: the
/// rest takes its hold after 100 ms, and past 200 ms of pose the two trade
/// places every tick.
fn pose_graph(state: &str) -> BehaviorGraphDescriptor {
    let posed = || BehaviorActivityDescriptor {
        pose: Some("overhead".to_string()),
        stagger_to: None,
        sound: None,
        animation: Some(state.to_string()),
        motion: Some(MotionVerb::Hold),
        action: None,
        on_enter: None,
        facing: None,
        reposition: None,
        attack_turn: None,
        speed: None,
        layers: Default::default(),
    };
    let after = |to: &str, ms: f32| GuardedRow {
        release_after: None,
        queue: None,
        after_moment: false,
        to: to.to_string(),
        when: IrNode::Ge {
            a: Box::new(IrNode::Input {
                name: postretro_foundation::BRAIN_TIME_IN_ACTIVITY_MS_INPUT.to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(ms),
            }),
        },
    };
    let mut graph = enemy_behavior_graph();
    graph.move_speed = MOVE_SPEED;
    graph.envelope = BehaviorGraphEnvelope {
        initial: "overhead".to_string(),
        activities: std::collections::BTreeMap::from([
            ("overhead".to_string(), posed()),
            ("slam_overhead".to_string(), posed()),
        ]),
        transitions: std::collections::BTreeMap::from([
            ("overhead".to_string(), vec![after("slam_overhead", 100.0)]),
            ("slam_overhead".to_string(), vec![after("overhead", 200.0)]),
        ]),
    };
    graph
}

/// The animation presentation one peer shows: state, switch count and entry
/// stamp.
fn presented(registry: &EntityRegistry, entity: EntityId) -> (String, u32, Option<f64>) {
    let animation = registry
        .get_component::<MeshComponent>(entity)
        .expect("the enemy carries its mesh")
        .animation
        .clone()
        .expect("the mesh is animated");
    (
        animation.current_state.clone(),
        animation.switches.get(),
        animation.entered_at,
    )
}

// AC C2: entering an activity whose clip is already playing restarts nothing
// on the host or on a loopback client. A pose's rest and hold share a state,
// so the host never switches between them, nothing re-enters the wire, and the
// client keeps the clip it started. (This harness's clips never resolve, so
// the host half is proven with resolved clips in `postretro-ai`'s
// `entering_an_activity_on_the_playing_clip_restarts_nothing`.)
#[test]
fn moving_within_a_pose_restarts_no_clip_on_host_or_loopback_client() {
    const STATE: &str = "smash_windup";
    let graph = pose_graph(STATE);
    let states = paced_states(STATE, None);
    let mut h = EnemyReplicationHarness::new(perfect_link());
    h.descriptors = vec![paced_descriptor(&graph, &states)];
    spawn_host_player(&mut h.host_registry);
    let enemy = spawn_host_paced_enemy(&mut h.host_registry, &graph, &states, STATE);
    h.host_register_descriptors();
    let net_id = h.enemy_network_id(enemy);
    let mut host = HostTick::new();

    let mut host_baseline = None;
    let mut client_baseline = None;
    let mut activities = Vec::new();
    for _ in 0..60 {
        host.run(&mut h.host_registry);
        h.host_tick();
        h.client_receive_with_walk_playback();
        let activity = h
            .host_registry
            .get_component::<BrainComponent>(enemy)
            .expect("host enemy keeps its brain")
            .state_name()
            .expect("the brain sits in a declared activity")
            .to_string();
        if activities.last() != Some(&activity) {
            activities.push(activity);
        }

        let host_view = presented(&h.host_registry, enemy);
        assert_eq!(host_view.0, STATE);
        let host_seen = *host_baseline.get_or_insert(host_view.1);
        assert_eq!(host_view.1, host_seen, "the host never switches again");

        let Some(remote) = h.client_mapped_entity(net_id) else {
            continue;
        };
        let render_tick = f64::from(h.server_tick.saturating_sub(4));
        h.client_replication
            .sample_into_registry(&mut h.client_registry, render_tick, 1.0);
        let client_view = presented(&h.client_registry, remote);
        if client_view.0 != STATE {
            assert!(
                client_baseline.is_none(),
                "the client leaves the pose's clip"
            );
            continue;
        }
        let client_seen = client_baseline.get_or_insert((client_view.1, client_view.2));
        assert_eq!(
            (client_view.1, client_view.2),
            *client_seen,
            "the client neither switches nor re-stamps its clip"
        );
    }
    assert!(client_baseline.is_some(), "the client presents the pose");
    assert!(
        activities.len() >= 6,
        "the host moves between rest and hold repeatedly: {activities:?}"
    );
}
