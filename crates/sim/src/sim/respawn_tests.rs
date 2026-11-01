//! Respawn through authoritative tick start and the full simulation body.
use super::super::tests::{remote_command, sim_command, spawn_reload_pair, trigger_movement};
use super::super::*;
use glam::Quat;
use postretro_entities::components::{
    health::{HealthComponent, set_health_absolute},
    inventory::Inventory,
};
use postretro_entities::{
    DescriptorProvenance, DescriptorSpawnPath, LifeStatesDescriptor, RespawnPlacement,
    TeleporterComponent, Transform,
};
use postretro_foundation::{
    CrouchParams, DashParams, HealthDescriptor, LifeStatePosture, LifeStateProfile, MovementState,
    NO_TELEPORTER, PlayerMovementDescriptor, Seat,
};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

const DT: f32 = 1.0 / 60.0;
const EPS: f32 = 1.0e-4;

struct Rig {
    ctx: ScriptCtx,
    world: CollisionWorld,
}

fn player_descriptor() -> EntityTypeDescriptor {
    let m = trigger_movement();
    let mut movement = PlayerMovementDescriptor {
        capsule: m.capsule,
        ground: m.ground_params,
        air: m.air,
        fall: m.fall,
        dash: m.dash,
        crouch: m.crouch,
        slide: m.slide,
        swim: m.swim,
        vault: m.vault,
        view_feel: m.view_feel,
        sounds: m.sounds,
        knockback: m.knockback,
        forgiveness: None,
        stuck_stop_enabled: m.stuck_stop_enabled,
        stuck_stop_threshold: m.stuck_stop_threshold,
    };
    movement.air.jumps = 2;
    movement.crouch = Some(CrouchParams {
        half_height: 0.4,
        eye_height: 0.2,
        transition_rate: 15.0,
    });
    movement.dash = Some(DashParams {
        boost_speed: 18.0.into(),
        momentum_retention: 0.0.into(),
        steer_control: 0.0.into(),
        dash_drag: 18.0.into(),
        cooldown_ms: 250.0.into(),
        air_dashes: 2,
        preserve_vertical: false.into(),
    });
    let mut down = LifeStateProfile::default();
    down.movement.posture = LifeStatePosture::Crouch;
    down.movement.abilities = false;
    EntityTypeDescriptor {
        canonical_name: Some("player".into()),
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: Some(HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        }),
        behavior: None,
        movement: Some(movement),
        life_states: Some(LifeStatesDescriptor {
            initial: "alive".into(),
            states: [
                ("alive".into(), LifeStateProfile::default()),
                ("down".into(), down),
                ("spectating".into(), LifeStateProfile::default()),
            ]
            .into(),
            rejoin: Default::default(),
            state_ids: Default::default(),
            table_revision: 0,
        }),
    }
}

impl Rig {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        ctx.gravity.set(-20.0);
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![player_descriptor()]);
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-200.0, 0.0, -200.0),
                Vec3::new(200.0, 0.0, -200.0),
                Vec3::new(200.0, 0.0, 200.0),
                Vec3::new(-200.0, 0.0, 200.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        );
        ctx.respawn_placements.borrow_mut().map_order = [0.0, 8.0, 16.0]
            .into_iter()
            .enumerate()
            .map(|(map_index, x)| RespawnPlacement {
                map_index,
                foot_anchor: Vec3::X * x,
                yaw: std::f32::consts::FRAC_PI_2,
            })
            .collect();
        ctx.respawn_placements.borrow_mut().first_player = Some(0);
        Self { ctx, world }
    }

    fn spawn(&self, seat: u16, position: Vec3, local: bool) -> (EntityId, EntityId) {
        let data = self.ctx.data_registry.borrow();
        let descriptor = &data.entities[0];
        let mut movement =
            PlayerMovementComponent::from_descriptor(descriptor.movement.as_ref().unwrap());
        let states = descriptor.life_states.as_ref().unwrap();
        let (id, profile) = states.initial_state();
        postretro_foundation::enter_life_state(&mut movement, id, profile, states.table_revision);
        let mut registry = self.ctx.registry.borrow_mut();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 20, 100, 10);
        registry.set_component(pawn, movement).unwrap();
        registry
            .set_component(
                pawn,
                HealthComponent::from_descriptor(descriptor.health.as_ref().unwrap()),
            )
            .unwrap();
        registry
            .set_component(
                pawn,
                Transform {
                    position,
                    rotation: Quat::from_rotation_y(0.3),
                    ..Default::default()
                },
            )
            .unwrap();
        registry
            .set_component(
                pawn,
                DescriptorProvenance {
                    canonical_name: "player".into(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::PlayerSpawn,
                },
            )
            .unwrap();
        registry.bind_pawn_seat(pawn, Seat(seat));
        if local {
            registry.mark_local_player_pawn(pawn).unwrap();
        }
        self.ctx
            .respawn_placements
            .borrow_mut()
            .seats
            .insert(Seat(seat), usize::from(seat).min(2));
        (pawn, weapon)
    }

    fn enqueue(&self, pawn: EntityId, health: Option<f32>, state: Option<&str>) {
        crate::respawn_commands::enqueue_respawns(
            &self.ctx,
            &self.ctx.registry.borrow(),
            &[pawn],
            health,
            state,
        );
    }
    fn movement(&self, pawn: EntityId) -> PlayerMovementComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone()
    }
    fn pose(&self, pawn: EntityId) -> Transform {
        *self
            .ctx
            .registry
            .borrow()
            .get_component::<Transform>(pawn)
            .unwrap()
    }
    fn health(&self, pawn: EntityId) -> HealthComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .clone()
    }
    fn name(&self, pawn: EntityId) -> String {
        let movement = self.movement(pawn);
        self.ctx
            .data_registry
            .borrow()
            .life_state_names
            .get(usize::from(movement.life_state))
            .cloned()
            .unwrap_or_default()
    }
    fn start(
        &self,
        facing: impl FnMut(EntityId) -> Option<f32>,
    ) -> (StartedTick, Vec<RespawnTurn>) {
        let pending = begin_tick(&self.ctx.registry, Some(&self.ctx));
        finish_tick_start(
            pending,
            &mut self.ctx.registry.borrow_mut(),
            &self.world,
            facing,
        )
    }
    fn tick(&self, command: &SimCommand, remote: &[RemotePawnCommand]) -> TickEvents {
        let mut progress = ProgressTracker::new();
        let mut movers = MoverTickStateTable::default();
        let mut system = TriggerSystem::default();
        let bridge = TriggerVolumeBridge::new();
        let bindings = TriggerBindingTable::default();
        let edges = HashMap::new();
        simulate_tick(
            self.ctx.registry.clone(),
            &self.world,
            &HitZoneStore::new(),
            None,
            EnvironmentView::level_only(-20.0),
            None,
            0.0,
            &mut progress,
            |_, _, _, _| AiTickResult {
                events: Vec::new(),
                projectile_spawns: Vec::new(),
            },
            &[],
            &mut movers,
            remote,
            command,
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            Some(TriggerTickContext {
                teleporters: None,
                system: &mut system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: self.ctx.slot_table.clone(),
                script_ctx: Some(self.ctx.clone()),
                auto_close_timers: None,
                use_edges: &edges,
            }),
            |_| {},
        )
    }
}

#[test]
fn respawn_tick_start_resets_crouched_geometry_motion_budgets_and_all_presented_alphas() {
    let rig = Rig::new();
    let (pawn, weapon) = rig.spawn(0, Vec3::new(70.0, 4.0, 5.0), true);
    let charge: postretro_foundation::WeaponDescriptor = serde_json::from_value(json!({"damage":5,"range":20,"resolution":"hitscan","primary":{"trigger":"press","recoveryMs":10,"charge":{"minMs":10,"fullMs":2000},"steps":[{"kind":"shot"}]}})).unwrap();
    rig.ctx
        .registry
        .borrow_mut()
        .set_component(weapon, WeaponComponent::from_descriptor(&charge))
        .unwrap();
    rig.tick(&sim_command(true, false), &[]);
    assert!(
        rig.ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .state
            .activation_cursor()
            .is_some()
    );
    crate::life_state_commands::set_life_state(
        &rig.ctx,
        &mut rig.ctx.registry.borrow_mut(),
        &[pawn],
        "down",
    );
    let mut movement = rig.movement(pawn);
    movement.capsule.half_height = 0.4;
    movement.capsule.eye_height = 0.2;
    movement.movement_state = MovementState::Sliding {
        elapsed_ms: 80.0,
        boost: Vec3::X * 20.0,
        eye_current: 0.2,
    };
    movement.velocity = Vec3::splat(10.0);
    movement.knockback_velocity = Vec3::splat(3.0);
    movement.push_velocity = Vec3::splat(2.0);
    movement.ground = postretro_foundation::GroundRef::Mover(99);
    movement.air_jumps_remaining = 0;
    movement.air_dashes_remaining = 0;
    movement.dash_cooldown_ms = 100.0;
    movement.coyote_timer_ms = 100.0;
    movement.jump_buffer_timer_ms = 100.0;
    movement.jump_spent = true;
    movement.swim_reentry_latched = true;
    movement.teleport.generation = 4;
    movement.teleport.last_teleporter = 0;
    movement.teleport.accumulated_yaw = 0.1;
    let inventory = rig
        .ctx
        .registry
        .borrow()
        .get_component::<Inventory>(pawn)
        .unwrap()
        .clone();
    let mut health = rig.health(pawn);
    health.current = 0.0;
    health.death_handled = true;
    {
        let mut r = rig.ctx.registry.borrow_mut();
        r.set_component(pawn, movement).unwrap();
        r.set_component(pawn, health).unwrap();
        r.set_component(
            pawn,
            postretro_foundation::ImmersionComponent { immersion: 1.0 },
        )
        .unwrap();
    }
    rig.enqueue(pawn, None, None);
    let old = rig.pose(pawn);
    let (_, turns) = rig.start(|_| Some(0.3));
    let movement = rig.movement(pawn);
    assert_eq!(movement.velocity, Vec3::ZERO);
    assert_eq!(movement.knockback_velocity, Vec3::ZERO);
    assert_eq!(movement.push_velocity, Vec3::ZERO);
    assert!(matches!(movement.movement_state, MovementState::Normal));
    assert_eq!(movement.capsule.half_height, 0.8);
    assert_eq!(movement.capsule.eye_height, 0.5);
    assert_eq!(
        rig.ctx
            .registry
            .borrow()
            .get_component::<postretro_foundation::ImmersionComponent>(pawn)
            .unwrap()
            .immersion,
        0.0
    );
    assert_eq!(
        rig.pose(pawn).position,
        Vec3::new(0.0, 1.2, 0.0),
        "spawn foot anchor uses restored standing geometry"
    );
    assert_ne!(rig.pose(pawn).position, old.position);
    assert_eq!(movement.ground, postretro_foundation::GroundRef::World);
    assert_eq!(
        (movement.air_jumps_remaining, movement.air_dashes_remaining),
        (2, 2)
    );
    assert_eq!(
        (
            movement.dash_cooldown_ms,
            movement.coyote_timer_ms,
            movement.jump_buffer_timer_ms
        ),
        (0.0, 0.0, 0.0)
    );
    assert!(!movement.jump_spent && !movement.swim_reentry_latched);
    assert_eq!(
        (
            movement.teleport.generation,
            movement.teleport.last_teleporter
        ),
        (5, NO_TELEPORTER)
    );
    assert!((turns[0].yaw_delta - (std::f32::consts::FRAC_PI_2 - 0.3)).abs() < EPS);
    assert_eq!(rig.name(pawn), "alive");
    assert_eq!(movement.life_state_elapsed_ticks, 0);
    assert_eq!(rig.health(pawn).current, 100.0);
    assert!(!rig.health(pawn).death_handled);
    let r = rig.ctx.registry.borrow();
    assert_eq!(
        r.get_component::<Inventory>(pawn).unwrap().wieldables,
        inventory.wieldables
    );
    assert!(
        r.get_component::<WeaponComponent>(weapon)
            .unwrap()
            .state
            .activation_cursor()
            .is_none()
    );
    for alpha in [0.0, 0.25, 0.5, 0.75, 1.0] {
        assert_eq!(
            r.interpolated_transform(pawn, alpha).unwrap().position,
            Vec3::new(0.0, 1.2, 0.0)
        );
        assert!(
            (r.interpolated_transform(pawn, alpha).unwrap().rotation * Vec3::NEG_Z - Vec3::NEG_X)
                .length()
                < EPS
        );
    }
    drop(r);
    set_health_absolute(&mut rig.ctx.registry.borrow_mut(), pawn, 0.0);
    assert!(
        rig.tick(&sim_command(false, false), &[])
            .death
            .iter()
            .any(|event| event == "playerDied"),
        "respawn re-arms the death edge"
    );
}

#[test]
fn respawn_full_tick_turns_forward_input_and_integrates_once_from_rest() {
    for (remote, legacy) in [(false, false), (true, false), (false, true)] {
        let rig = Rig::new();
        if remote {
            rig.spawn(0, Vec3::new(100.0, 1.2, 0.0), true);
        }
        let (pawn, weapon) = rig.spawn(
            u16::from(remote),
            Vec3::new(60.0, 1.2, 0.0),
            !remote && !legacy,
        );
        let mut dirty = rig.movement(pawn);
        dirty.velocity = Vec3::splat(25.0);
        rig.ctx
            .registry
            .borrow_mut()
            .set_component(pawn, dirty)
            .unwrap();
        rig.enqueue(pawn, None, None);
        let mut command = sim_command(false, false);
        command.movement.wish_dir = glam::Vec2::Y;
        command.movement.facing_yaw = 0.3;
        if remote {
            let mut received = remote_command(pawn, Some(weapon), 1, 1, false, false);
            received.command = command;
            rig.tick(&sim_command(false, false), &[received]);
        } else if legacy {
            assert!(rig.ctx.registry.borrow().local_player_pawn().is_none());
            assert_eq!(rig.ctx.registry.borrow().seat_for_pawn(pawn), Some(Seat(0)));
            // Nonempty remote inputs must not hide the unmarked local fallback.
            let (other, other_weapon) = rig.spawn(1, Vec3::new(100.0, 1.2, 0.0), false);
            rig.tick(
                &command,
                &[remote_command(
                    other,
                    Some(other_weapon),
                    2,
                    1,
                    false,
                    false,
                )],
            );
        } else {
            rig.tick(&command, &[]);
        }
        let movement = rig.movement(pawn);
        assert!(
            (movement.velocity - Vec3::NEG_X * (7.0 * 10.0 * DT)).length() < EPS,
            "one ground acceleration tick along placement yaw, remote={remote}, legacy={legacy}: {:?}",
            movement.velocity
        );
        assert!(((rig.pose(pawn).rotation * Vec3::NEG_Z) - Vec3::NEG_X).length() < EPS);
        assert_eq!(movement.life_state_elapsed_ticks, 1);
    }
}

#[test]
fn respawn_seat_order_counts_later_queued_pawns_then_earlier_relocations() {
    let rig = Rig::new();
    let (first, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
    let (later, _) = rig.spawn(1, Vec3::new(0.0, 1.2, 0.0), false);
    rig.spawn(2, Vec3::new(8.0, 1.2, 0.0), false);
    rig.enqueue(later, None, None);
    rig.enqueue(first, None, None);
    rig.start(|_| None);
    assert_eq!(
        rig.pose(first).position,
        Vec3::new(16.0, 1.2, 0.0),
        "later queued pawn still blocks its current position"
    );
    assert_eq!(
        rig.pose(later).position,
        Vec3::new(0.0, 1.2, 0.0),
        "earlier relocation occupies 16; later seat takes now-free lowest map index"
    );
}

#[test]
fn respawn_last_request_wins_and_adjacent_state_write_does_not_resurrect_early() {
    let rig = Rig::new();
    let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
    set_health_absolute(&mut rig.ctx.registry.borrow_mut(), pawn, 0.0);
    rig.enqueue(pawn, Some(25.0), Some("down"));
    crate::life_state_commands::set_life_state(
        &rig.ctx,
        &mut rig.ctx.registry.borrow_mut(),
        &[pawn],
        "down",
    );
    rig.enqueue(pawn, Some(40.0), Some("alive"));
    assert_eq!(rig.health(pawn).current, 0.0);
    assert_eq!(rig.name(pawn), "down");
    assert_eq!(rig.pose(pawn).position.x, 60.0);
    assert_eq!(rig.ctx.pending_respawns.borrow().len(), 1);
    rig.start(|_| None);
    assert_eq!(rig.health(pawn).current, 40.0);
    assert_eq!(rig.name(pawn), "alive");
    assert!(rig.ctx.pending_respawns.borrow().is_empty());
}

#[test]
fn respawn_invalid_health_warns_and_clamps_including_direct_nan() {
    for invalid in [0.0, -1.0, 101.0, f32::NAN, f32::INFINITY] {
        let logs = LogCapture::start();
        let rig = Rig::new();
        let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
        rig.enqueue(pawn, Some(invalid), None);
        rig.start(|_| None);
        assert_eq!(rig.health(pawn).current, 100.0);
        logs.assert_logged_once(log::Level::Warn, "invalid health");
    }
}

#[test]
fn respawn_named_state_resolves_after_reload_or_falls_back_once() {
    for case in 0..3 {
        let logs = LogCapture::start();
        let rig = Rig::new();
        let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
        rig.enqueue(pawn, None, Some("spectating"));
        let mut next = rig.ctx.data_registry.borrow().entities.clone();
        match case {
            0 => {
                next[0]
                    .life_states
                    .as_mut()
                    .unwrap()
                    .states
                    .insert("aardvark".into(), LifeStateProfile::default());
            }
            1 => {
                next[0]
                    .life_states
                    .as_mut()
                    .unwrap()
                    .states
                    .remove("spectating");
            }
            _ => next[0].life_states = None,
        }
        rig.ctx
            .data_registry
            .borrow_mut()
            .replace_entity_types(next);
        assert_eq!(
            rig.ctx.pending_respawns.borrow().len(),
            1,
            "reload retains the pending edge"
        );
        rig.start(|_| None);
        assert_eq!(
            rig.name(pawn),
            match case {
                0 => "spectating",
                1 => "alive",
                _ => "",
            }
        );
        assert_eq!(rig.movement(pawn).life_state_elapsed_ticks, 0);
        assert_eq!(rig.pose(pawn).position, Vec3::new(0.0, 1.2, 0.0));
        match case {
            0 => logs.assert_not_logged(log::Level::Warn, "Respawn"),
            1 => logs.assert_logged_once(log::Level::Warn, "removed before drain"),
            _ => logs.assert_logged_once(log::Level::Warn, "no longer declares"),
        }
    }
}

#[test]
fn respawn_missing_request_target_skips_once_and_no_free_placement_warns_once() {
    {
        let logs = LogCapture::start();
        let rig = Rig::new();
        let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
        rig.enqueue(pawn, None, None);
        rig.ctx.registry.borrow_mut().despawn(pawn).unwrap();
        rig.start(|_| None);
        assert!(rig.ctx.pending_respawns.borrow().is_empty());
        logs.assert_logged_once(log::Level::Warn, "no longer participates");
    }
    {
        let logs = LogCapture::start();
        let rig = Rig::new();
        let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
        for (seat, x) in [(1, 0.0), (2, 8.0), (3, 16.0)] {
            rig.spawn(seat, Vec3::new(x, 1.2, 0.0), false);
        }
        rig.enqueue(pawn, None, None);
        rig.start(|_| None);
        assert_eq!(rig.pose(pawn).position, Vec3::new(0.0, 1.2, 0.0));
        logs.assert_logged_once(log::Level::Warn, "no free placement");
    }
}

#[test]
fn respawn_latches_teleporter_armed_in_the_same_tick_start_until_reentry() {
    use postretro_level_format::teleporters::TeleportersSection;
    let mut rig = Rig::new();
    let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
    let record = super::super::teleport_tick_tests::classic(
        Vec3::new(0.0, 1.2, 0.0),
        3.0,
        Vec3::new(30.0, 1.2, 0.0),
        false,
    );
    rig.world.teleporters =
        postretro_physics::teleporters::Teleporters::from_section(&TeleportersSection {
            teleporters: vec![record],
        });
    let gate = rig.ctx.registry.borrow_mut().spawn(Transform::default());
    rig.ctx
        .registry
        .borrow_mut()
        .set_component(
            gate,
            TeleporterComponent {
                index: 0,
                source_entity: 0,
                armed: false,
                arrival_sound: String::new(),
                on_teleport: String::new(),
            },
        )
        .unwrap();
    rig.world.teleporters.bind_entity(0, gate);
    rig.ctx
        .pending_teleporter_mutations
        .borrow_mut()
        .push((gate, true));
    rig.enqueue(pawn, None, None);
    assert!(
        rig.tick(&sim_command(false, false), &[])
            .teleports
            .is_empty()
    );
    assert_eq!(rig.movement(pawn).teleport.arrival_latches, [0]);
    let mut pose = rig.pose(pawn);
    pose.position.x = 10.0;
    rig.ctx
        .registry
        .borrow_mut()
        .set_component(pawn, pose)
        .unwrap();
    rig.tick(&sim_command(false, false), &[]);
    assert!(rig.movement(pawn).teleport.arrival_latches.is_empty());
    pose.position.x = 0.0;
    rig.ctx
        .registry
        .borrow_mut()
        .set_component(pawn, pose)
        .unwrap();
    assert_eq!(rig.tick(&sim_command(false, false), &[]).teleports.len(), 1);
    assert!((rig.pose(pawn).position - Vec3::new(30.0, 1.2, 0.0)).length() < EPS);
}

#[test]
fn respawn_target_posture_keeps_spawn_feet_and_drops_active_dash_or_vault() {
    for (old, target, height, eye) in [
        (
            MovementState::Dash {
                elapsed_ms: 20.0,
                boost: Vec3::X * 18.0,
            },
            "down",
            0.4,
            0.2,
        ),
        (
            MovementState::Vault {
                elapsed_ms: 20.0,
                target: Vec3::new(4.0, 1.0, 0.0),
                start_offset: Vec3::NEG_X,
                mover: None,
                entry_speed: 11.0,
            },
            "alive",
            0.8,
            0.5,
        ),
    ] {
        let rig = Rig::new();
        let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
        let mut movement = rig.movement(pawn);
        movement.movement_state = old;
        movement.velocity = Vec3::X * 18.0;
        rig.ctx
            .registry
            .borrow_mut()
            .set_component(pawn, movement)
            .unwrap();
        rig.enqueue(pawn, None, Some(target));
        rig.start(|_| None);
        let movement = rig.movement(pawn);
        assert_eq!(movement.capsule.half_height, height);
        assert_eq!(movement.capsule.eye_height, eye);
        assert_eq!(
            rig.pose(pawn).position,
            Vec3::Y * (height + movement.capsule.radius)
        );
        assert_eq!(movement.ground, postretro_foundation::GroundRef::World);
        assert_eq!(movement.velocity, Vec3::ZERO);
        assert!(!matches!(
            movement.movement_state,
            MovementState::Dash { .. }
                | MovementState::Sliding { .. }
                | MovementState::Vault { .. }
        ));
        assert_eq!(rig.name(pawn), target);
        assert_eq!(movement.life_state_elapsed_ticks, 0);
    }
}

#[path = "respawn_boundary_tests.rs"]
mod boundaries;

// Regression: VM NaN/infinities failed load before AC13's drain normalization.
#[test]
fn respawn_true_vm_nonfinite_health_warns_once_at_drain_restores_max_and_rearms_death() {
    use postretro_entities::ReactionDescriptor;
    use postretro_scripting_core::data_descriptors::{
        named_reaction_from_js, named_reaction_from_lua,
    };
    use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

    for (js_number, lua_number) in [
        ("NaN", "0/0"),
        ("Infinity", "math.huge"),
        ("-Infinity", "-math.huge"),
    ] {
        for sequence in [false, true] {
            let js_command =
                format!("{{primitive:'respawn',kind:'player',args:{{health:{js_number}}}}}");
            let lua_command =
                format!("{{primitive='respawn',kind='player',args={{health={lua_number}}}}}");
            let js_source = if sequence {
                format!("({{name:'restore',sequence:[{js_command}]}})")
            } else {
                format!("({{name:'restore',...{js_command}}})")
            };
            let lua_source = if sequence {
                format!("return {{name='restore',sequence={{{lua_command}}}}}")
            } else {
                format!("local r={lua_command}; r.name='restore'; return r")
            };
            let runtime = rquickjs::Runtime::new().unwrap();
            let context = rquickjs::Context::full(&runtime).unwrap();
            let js = context.with(|ctx| {
                let value = ctx.eval(js_source).unwrap();
                named_reaction_from_js(&ctx, value).unwrap()
            });
            let lua = mlua::Lua::new();
            let value = lua.load(&lua_source).eval().unwrap();
            let lua = named_reaction_from_lua(value).unwrap();
            for parsed in [js, lua] {
                let logs = LogCapture::start();
                let rig = Rig::new();
                let (pawn, _) = rig.spawn(0, Vec3::new(60.0, 1.2, 0.0), true);
                let mut health = rig.health(pawn);
                health.current = 0.0;
                health.death_handled = true;
                rig.ctx
                    .registry
                    .borrow_mut()
                    .set_component(pawn, health)
                    .unwrap();
                match parsed.descriptor {
                    ReactionDescriptor::Primitive(body) => {
                        let mut registry = ReactionPrimitiveRegistry::new();
                        crate::respawn_commands::register_respawn_reaction_primitives(
                            &mut registry,
                            rig.ctx.clone(),
                        );
                        registry.get("respawn").unwrap()(
                            &mut rig.ctx.registry.borrow_mut(),
                            &[pawn],
                            &body.args,
                        )
                        .unwrap();
                    }
                    ReactionDescriptor::Sequence(steps) => {
                        let mut registry = SequencedPrimitiveRegistry::new();
                        crate::respawn_commands::register_sequenced_respawn_primitives(
                            &mut registry,
                            rig.ctx.clone(),
                        );
                        registry.get("respawn").unwrap()(pawn, &steps[0].args).unwrap();
                    }
                    other => panic!("unexpected respawn descriptor: {other:?}"),
                }
                assert!(rig.health(pawn).current.abs() < EPS);
                logs.assert_not_logged(log::Level::Warn, "invalid health");
                let _ = rig.start(|_| None);
                assert!((rig.health(pawn).current - rig.health(pawn).max).abs() < EPS);
                assert!(!rig.health(pawn).death_handled);
                assert!(rig.ctx.pending_respawns.borrow().is_empty());
                logs.assert_logged_once(log::Level::Warn, "invalid health");
                set_health_absolute(&mut rig.ctx.registry.borrow_mut(), pawn, 0.0);
                assert!(
                    rig.tick(&sim_command(false, false), &[])
                        .death
                        .iter()
                        .any(|event| event == "playerDied")
                );
            }
        }
    }
}
