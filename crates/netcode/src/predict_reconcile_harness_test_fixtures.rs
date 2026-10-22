// Shared data builders for the prediction/reconciliation harnesses: the link
// profiles, worlds, the player descriptor both peers load, commands, bystanders and
// the host tick trace. `LoopbackHarness` itself lives in
// `predict_reconcile_loopback_test_fixtures` and is re-exported here.
// See: context/lib/networking.md §Testing the conditioned link · context/lib/testing_guide.md §4
//
// Test infrastructure only (`#[cfg(test)]`), not production runtime state. The
// Transform-only bystanders spawned here live only in test paths and never carry
// `local_player`.

#![cfg(test)]

use glam::{Vec2, Vec3};

use postretro_collision::{ConvexLeaf, EnvironmentVolume, EnvironmentVolumes, LeafPlane};
use postretro_net::harness::LinkConfig;
use postretro_net::wire::{InputCommand, WireFireButtonState, WireMovementInput};

use super::command_queue::ResolvedPawnCommand;
use super::predict_reconcile_relay_link_test_fixtures::dev_profile;
use crate::collision::CollisionWorld;
use crate::kinematic_mover::{MoverTickStateTable, run_kinematic_mover_tick};
use crate::movement::MovementInput;
use crate::sim::SimCommand;
use crate::weapon::FireButtonState;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, EntityTypeDescriptor, Transform,
};
use postretro_foundation::{
    AirParams, BoolOrIr, CapsuleParams, CrouchParams, DashParams, FallParams, ForgivenessParams,
    GroundParams, HealthDescriptor, NumberOrIr, PlayerMovementComponent, PlayerMovementDescriptor,
    SlideParams, SpeedParams,
};

pub(crate) const DT: f32 = 1.0 / 60.0;
/// "No corrections": every recorded correction is at
/// most this many metres under [`light_link`].
pub(crate) const NO_CORRECTION_M: f32 = 1.0e-4;
/// How far a settled client pawn may sit from the host's once both are at rest
/// after a run under a lossy link.
pub(crate) const SETTLED_PAWN_TOLERANCE_M: f32 = 0.05;
pub(crate) const GRAVITY: f32 = -20.0;
pub(crate) const CLIENT_ID: u64 = 1;
pub(crate) const START: Vec3 = Vec3::new(0.0, 1.21, 0.0);
pub(crate) const MOVING_PLATFORM_ID: u32 = 77;
pub(crate) const MOVING_PLATFORM_SPEED_MPS: f32 = 0.6;
pub(crate) const ROTATING_PLATFORM_INITIAL_RATE_RAD_S: f32 = 1.2;
pub(crate) const ROTATING_PLATFORM_ACCEL_RAD_S2: f32 = 2.4;
pub(crate) const ROTATING_PLATFORM_RIDER_START: Vec3 = Vec3::new(1.0, 1.21, 0.0);

/// Settle margin, in host ticks, between the host's resolved cursor reaching the last
/// sent command tick and freezing the drain *target* tick. Covers the gap policy's
/// hold window (INPUT_HOLD_TICKS) plus enough ticks for neutral-input deceleration to
/// fall well below the gate tolerance, so the frozen-target snapshot reflects an
/// effectively-at-rest pose. Bounded + deterministic (vs. waiting for exact stillness,
/// whose geometric decay tail is unboundedly slow). See `drain_step`.
pub(crate) const DRAIN_SETTLE_TICKS: u32 = 30;

/// The tick a harness host stamps on the snapshot it sends after `completed_ticks`
/// fixed ticks. Production's `net_serialize_and_send` hands `host_replicate` the
/// completed-tick counter plus one, and the client measures mover replay spans
/// against that stamp; a harness stamping the bare counter replays every mover one
/// tick further than production does.
pub(crate) fn production_snapshot_stamp(completed_ticks: u32) -> u32 {
    completed_ticks.wrapping_add(1)
}

/// The client's pre-predict mover tick, as production's
/// `client_predict_loaded_movers_tick` runs it: only `KinematicMover` entities take
/// previous = current before the driver steps them. The local pawn's stamp belongs
/// to `client_predict_tick`, so a registry-wide stamp here would hide a missing one.
pub(crate) fn client_tick_loaded_movers(
    registry: &mut EntityRegistry,
    mover_states: &mut MoverTickStateTable,
    dt: f32,
) {
    let movers: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .map(|(id, _)| id)
        .collect();
    for mover in movers {
        registry.snapshot_transform(mover);
    }
    run_kinematic_mover_tick(registry, mover_states, dt);
}

/// The dev profile ([`dev_profile`]) at a fixed seed, for the loopback
/// scenarios that take one link config for both directions: a 45..105 ms
/// one-way range, about 150 ms mean RTT, 5% loss.
pub(crate) fn mandated_link() -> LinkConfig {
    dev_profile(0x1502)
}

/// A near-perfect link for scenario tests that exercise the gap policy, stale and
/// duplicate seams, and exact prediction deterministically without the full
/// latency profile's loss obscuring the assertion. A small fixed delay keeps the
/// predict→ack loop realistic (the client predicts ahead of the host) without
/// jitter or loss.
pub(crate) fn light_link() -> LinkConfig {
    LinkConfig {
        delay: 32,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x6010,
    }
}

/// A large flat floor so the pawn stays grounded for the whole run — keeps the
/// scenario about prediction/reconciliation, not terrain interaction.
pub(crate) fn floor_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-500.0, 0.0, -500.0),
        Vec3::new(500.0, 0.0, -500.0),
        Vec3::new(500.0, 0.0, 500.0),
        Vec3::new(-500.0, 0.0, 500.0),
    ];
    let triangles = vec![[0, 2, 1], [0, 3, 2]];
    CollisionWorld::from_triangles_for_test(points, triangles)
}

/// Flat floor joined continuously to a downhill facet at `z = -1`. Forward
/// movement crosses the crease, so prediction can hold a newer contact normal
/// than an authoritative baseline restored on the flat facet.
pub(crate) fn faceted_slope_world(slope: f32) -> CollisionWorld {
    let far_y = slope * (-500.0 + 1.0);
    let points = vec![
        Vec3::new(-500.0, far_y, -500.0),
        Vec3::new(500.0, far_y, -500.0),
        Vec3::new(500.0, 0.0, -1.0),
        Vec3::new(-500.0, 0.0, -1.0),
        Vec3::new(-500.0, 0.0, -1.0),
        Vec3::new(500.0, 0.0, -1.0),
        Vec3::new(500.0, 0.0, 500.0),
        Vec3::new(-500.0, 0.0, 500.0),
    ];
    CollisionWorld::from_triangles_for_test(
        points,
        vec![[0, 2, 1], [0, 3, 2], [4, 6, 5], [4, 7, 6]],
    )
}

pub(crate) fn faceted_floor_height(slope: f32, z: f32) -> f32 {
    if z < -1.0 { slope * (z + 1.0) } else { 0.0 }
}

pub(crate) fn downhill_facet_normal(slope: f32) -> Vec3 {
    Vec3::new(0.0, 1.0, -slope).normalize()
}

/// An axis-aligned box environment volume that sets only gravity, as the level
/// loader would hand both peers.
pub(crate) fn gravity_box_volume(min: Vec3, max: Vec3, gravity: f32) -> EnvironmentVolume {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: Some(gravity),
        push: None,
        force: 1.0,
        fluid: None,
        brushes: vec![ConvexLeaf {
            planes: vec![
                plane(Vec3::X, max.x),
                plane(Vec3::NEG_X, -min.x),
                plane(Vec3::Y, max.y),
                plane(Vec3::NEG_Y, -min.y),
                plane(Vec3::Z, max.z),
                plane(Vec3::NEG_Z, -min.z),
            ],
            aabb_min: min,
            aabb_max: max,
        }],
    }
}

/// One level volume record as every peer loads it from the level file: its
/// fluid by NAME. `volume.fluid` is ignored; each peer resolves `fluid` against
/// its own active fluid table ([`resolve_environment_records`]).
#[derive(Debug, Clone)]
pub(crate) struct NamedEnvironmentVolume {
    pub(crate) volume: EnvironmentVolume,
    pub(crate) fluid: Option<String>,
}

/// Resolve level records against a fluid table, as the binary's level install
/// does: a fluid's index is its position in `fluid_names`, and a name the table
/// lacks loads dry while the volume's gravity and push still apply.
pub(crate) fn resolve_environment_records(
    records: &[NamedEnvironmentVolume],
    fluid_names: &[String],
) -> EnvironmentVolumes {
    EnvironmentVolumes::build(
        records
            .iter()
            .map(|record| EnvironmentVolume {
                fluid: record
                    .fluid
                    .as_deref()
                    .and_then(|name| fluid_names.iter().position(|declared| declared == name))
                    .and_then(|index| u16::try_from(index).ok()),
                ..record.volume.clone()
            })
            .collect(),
    )
}

/// The shared player descriptor (dash-capable) both ends materialize their pawn
/// from. Both peers load identical local descriptors, so movement tuning comes from
/// the shared table; the host's level gravity alone reaches the client through
/// Control-channel `TuningPayload` delivery. Snapshots carry the mutable tick state.
pub(crate) fn player_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
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
        dash: Some(DashParams {
            boost_speed: NumberOrIr::Literal(18.0),
            momentum_retention: NumberOrIr::Literal(0.65),
            steer_control: NumberOrIr::Literal(0.2),
            dash_drag: NumberOrIr::Literal(18.0),
            cooldown_ms: NumberOrIr::Literal(250.0),
            air_dashes: 0,
            preserve_vertical: BoolOrIr::Literal(false),
        }),
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: Some(CrouchParams {
            half_height: 0.4,
            eye_height: 0.2,
            transition_rate: 15.0,
        }),
        slide: Some(SlideParams {
            min_speed: 8.0,
            slide_drag: 2.0,
            slope_assist: 1.0,
            steer_rate: 180.0,
            entry_boost: 1.5,
            min_duration_ms: 250.0,
        }),
        swim: None,
        vault: None,
        view_feel: None,
    }
}

pub(crate) fn component() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&player_descriptor())
}

/// The descriptor class both peers share for the local pawn (mirrors the production
/// default the host stamps and the client falls back to).
pub(crate) const ENTITY_CLASS: &str = "player";

/// The shared descriptor table both peers load (same content on both ends). The host
/// materializes its authoritative pawn from this table's `movement` block; the client
/// materializes its LOCAL pawn's component from the SAME table via the real
/// `materialize_net_local_movement_component` path — so the two components are
/// bit-identical and the wire mutable subset merges onto a matching base. The single
/// `"player"` descriptor wraps the harness `player_descriptor()` movement block.
pub(crate) fn entity_descriptors() -> Vec<EntityTypeDescriptor> {
    vec![EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(ENTITY_CLASS.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: Some(player_descriptor()),
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }]
}

/// The `DescriptorProvenance` a real net-slot pawn carries, so the host snapshot
/// production (`produce_owned_snapshots` -> `descriptor_entity_class`) stamps
/// `entity_class = Some("pawn:player")` on the owned snapshot — exactly as
/// `spawn_net_slot_pawn` does in production.
pub(crate) fn net_slot_provenance() -> DescriptorProvenance {
    DescriptorProvenance {
        canonical_name: ENTITY_CLASS.to_string(),
        owned_components: [DescriptorComponentKind::Movement].into_iter().collect(),
        map_overrides: Default::default(),
        spawn_path: DescriptorSpawnPath::NetworkSlot,
    }
}

/// A forward-walking sim command at the given facing, dash optional. The harness
/// stamps the wire `client_tick` from `ClientPrediction::next_client_tick`.
pub(crate) fn forward_command(dash_pressed: bool) -> SimCommand {
    SimCommand {
        input_tick: 0,
        secondary_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: postretro_foundation::ActivationInput::default(),
        movement: MovementInput {
            wish_dir: Vec2::new(0.0, 1.0),
            jump_pressed: false,
            dash_pressed,
            running: true,
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

pub(crate) fn idle_command() -> SimCommand {
    SimCommand {
        input_tick: 0,
        secondary_button: crate::weapon::FireButtonState {
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

/// One remote Use rising edge. Keep the full command and movement mirror in
/// lockstep so the client-prediction and client-to-host input boundaries exercise
/// the same authored edge.
pub(crate) fn use_command() -> SimCommand {
    let mut command = idle_command();
    command.movement.use_pressed = true;
    command.use_pressed = true;
    command
}

/// An owned `InputCommand` directly at a chosen `client_tick` — used by the
/// scenario tests that send duplicate or malformed input on the client's framed
/// Input channel ([`LoopbackHarness::client_send_message`]).
pub(crate) fn input_at(client_tick: u32, wish_forward: f32) -> InputCommand {
    InputCommand {
        teleport_yaw_stamp: 0.0,
        secondary_button: postretro_net::wire::WireFireButtonState {
            pressed: false,
            active: false,
        },
        activation: postretro_net::wire::WireActivationInput::default(),
        client_tick,
        movement: WireMovementInput {
            wish_dir: [0.0, wish_forward],
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
            aim_pitch: 0.0,
            firing_slot: 0,
        },
        fire_button: WireFireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
    }
}

/// A bystander entity (no `PlayerMovement`, no `local_player`) carrying a
/// zero-HP `HealthComponent`. If the full `simulate_tick` death sweep ran during
/// prediction/reconciliation it would despawn this entity; its survival is the
/// observable seam proving the movement-only path never reached the registry-wide
/// systems (testing_guide: assert behaviour at the seam).
pub(crate) fn spawn_dead_bystander(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform {
        position: Vec3::new(50.0, 1.0, 50.0),
        ..Transform::default()
    });
    let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
        max: 100.0,
        hitbox: None,
        zone_multipliers: std::collections::HashMap::new(),
    });
    health.current = 0.0; // would be swept by run_death_sweep if it ran
    registry.set_component(id, health).unwrap();
    id
}

/// One host fixed tick as the client's pawn ended it: the command resolution
/// that drove it (after the teleport-frame rotation) and the authoritative pose,
/// avatar yaw and teleport record.
#[derive(Debug, Clone)]
pub(crate) struct HostTickTrace {
    pub(crate) server_tick: u32,
    pub(crate) resolved: Option<ResolvedPawnCommand>,
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) teleport: postretro_foundation::TeleportState,
    /// The movement state the pawn ended the tick in, so a test can see the
    /// tick a slide or vault started on the host.
    pub(crate) movement_state: postretro_foundation::MovementStateKind,
}

impl HostTickTrace {
    pub(crate) fn capture(
        registry: &EntityRegistry,
        pawn: EntityId,
        server_tick: u32,
        resolved: Option<ResolvedPawnCommand>,
    ) -> Self {
        let transform = registry.get_component::<Transform>(pawn).unwrap();
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        // Yaw of `Quat::from_rotation_y(yaw)`: its forward is (-sin, 0, -cos).
        let forward = transform.rotation * Vec3::NEG_Z;
        Self {
            server_tick,
            resolved,
            position: transform.position,
            yaw: (-forward.x).atan2(-forward.z),
            teleport: movement.teleport.clone(),
            movement_state: movement.movement_state.kind(),
        }
    }
}

pub(crate) use super::predict_reconcile_loopback_test_fixtures::LoopbackHarness;
