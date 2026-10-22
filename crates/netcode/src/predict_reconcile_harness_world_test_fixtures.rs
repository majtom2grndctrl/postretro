// The worlds `LoopbackHarness` scenarios run in: environment volumes and fluid
// tables, level gravity, player tuning variants, and the movers (platforms, a moving
// ledge, lifts, a rotator) with their host-only triggers.
// See: context/lib/networking.md · context/lib/testing_guide.md §4
//
// Test infrastructure only (`#[cfg(test)]`). Each builder starts from
// `LoopbackHarness::new` and installs the same level on both peers, each peer
// binding its own entities, before the client joins.

#![cfg(test)]

use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolume};
use postretro_combat_model::FluidMovementTable;
use postretro_entities::{
    EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverMode, Transform,
    TriggerActivation, TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{FluidRegistry, PlayerMovementComponent, SwimParams, VaultParams};
use postretro_net::harness::LinkConfig;

use super::predict_reconcile_harness_test_fixtures::{
    MOVING_PLATFORM_ID, MOVING_PLATFORM_SPEED_MPS, NamedEnvironmentVolume,
    ROTATING_PLATFORM_ACCEL_RAD_S2, ROTATING_PLATFORM_INITIAL_RATE_RAD_S,
    ROTATING_PLATFORM_RIDER_START, resolve_environment_records,
};
use super::predict_reconcile_loopback_test_fixtures::LoopbackHarness;
use super::replication::host_register_loaded_movers;
use crate::collision::CollisionWorld;
use crate::collision::moving::MoverCollider;
use crate::netcode::client_environment_view;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;

impl LoopbackHarness {
    /// Install the same fluidless environment volume records on both peers,
    /// each building its own query set as each peer does from its own copy of
    /// the level.
    pub(crate) fn with_environment_volumes(
        link: LinkConfig,
        volumes: Vec<EnvironmentVolume>,
    ) -> Self {
        assert!(
            volumes.iter().all(|volume| volume.fluid.is_none()),
            "fluid volumes name their fluid: use `with_fluid_tables`"
        );
        let records = volumes
            .into_iter()
            .map(|volume| NamedEnvironmentVolume {
                volume,
                fluid: None,
            })
            .collect();
        Self::with_fluid_tables(
            link,
            records,
            FluidRegistry::default(),
            FluidRegistry::default(),
        )
    }

    /// Seed the host and the client with independent fluid declarations, as two
    /// peers on the same mod id whose manifests differ, over the same level
    /// records. The host's volumes resolve against its registry; the client's
    /// resolve against nothing (dry) until the host's table arrives through the
    /// tuning-payload install, and never against `client_fluids`.
    pub(crate) fn with_fluid_tables(
        link: LinkConfig,
        records: Vec<NamedEnvironmentVolume>,
        host_fluids: FluidRegistry,
        client_fluids: FluidRegistry,
    ) -> Self {
        let mut h = Self::new(link);
        h.environment_records = records;
        h.client_fluids = client_fluids;
        h.client_environment_volumes =
            resolve_environment_records(&h.environment_records, &h.client_volume_fluid_names);
        h.host_reload_fluids(host_fluids);
        h.host_environment_volumes =
            resolve_environment_records(&h.environment_records, h.host_fluid_table.names());
        h
    }

    /// Give both peers' shared player descriptor a swim block, as a mod whose
    /// player can swim, and rematerialize the host pawn from it. Call before the
    /// client arms: the client materializes its pawn from the host's tuning.
    pub(crate) fn with_player_swim(mut self, swim: SwimParams) -> Self {
        for descriptor in &mut self.descriptors {
            if let Some(movement) = descriptor.movement.as_mut() {
                movement.swim = Some(swim.clone());
            }
        }
        let movement = self.descriptors[0]
            .movement
            .as_ref()
            .expect("the shared player descriptor has movement");
        self.host_registry
            .set_component(
                self.host_pawn,
                PlayerMovementComponent::from_descriptor(movement),
            )
            .unwrap();
        self
    }

    /// Give both peers' shared player descriptor ground friction tuning and
    /// rematerialize the host pawn from it, as [`Self::with_player_swim`] does.
    pub(crate) fn with_player_ground_friction(mut self, friction: f32, stop_speed: f32) -> Self {
        for descriptor in &mut self.descriptors {
            if let Some(movement) = descriptor.movement.as_mut() {
                movement.ground.friction = friction;
                movement.ground.stop_speed = stop_speed;
            }
        }
        let movement = self.descriptors[0]
            .movement
            .as_ref()
            .expect("the shared player descriptor has movement");
        self.host_registry
            .set_component(
                self.host_pawn,
                PlayerMovementComponent::from_descriptor(movement),
            )
            .unwrap();
        self
    }

    /// Give both peers' shared player descriptor a vault block and
    /// rematerialize the host pawn from it, as [`Self::with_player_swim`] does.
    pub(crate) fn with_player_vault(mut self, vault: VaultParams) -> Self {
        for descriptor in &mut self.descriptors {
            if let Some(movement) = descriptor.movement.as_mut() {
                movement.vault = Some(vault.clone());
            }
        }
        let movement = self.descriptors[0]
            .movement
            .as_ref()
            .expect("the shared player descriptor has movement");
        self.host_registry
            .set_component(
                self.host_pawn,
                PlayerMovementComponent::from_descriptor(movement),
            )
            .unwrap();
        self
    }

    /// HOST: commit a manifest whose fluids are `fluids`, as a host reload does:
    /// the registry and its movement table are replaced, and the installed
    /// volumes re-resolve when the table's names changed. The next host frame's
    /// whole-struct tuning dedupe resends the new table.
    pub(crate) fn host_reload_fluids(&mut self, fluids: FluidRegistry) {
        let table = FluidMovementTable::from_registry(&fluids);
        if table.names() != self.host_fluid_table.names() {
            self.host_environment_volumes =
                resolve_environment_records(&self.environment_records, table.names());
        }
        self.host_fluids = fluids;
        self.host_fluid_table = table;
    }

    /// The host's tick view: its volumes under its live level gravity and the
    /// viscosities of the table they resolved against.
    pub(crate) fn host_environment_view(&self) -> EnvironmentView<'_> {
        EnvironmentView::new(
            self.host_level_gravity,
            &self.host_environment_volumes,
            self.host_fluid_table.viscosities(),
        )
    }

    /// The client's prediction and replay view, through the production
    /// `client_environment_view`. `None` before the host's tuning installs.
    pub(crate) fn client_environment_view(&self) -> Option<EnvironmentView<'_>> {
        client_environment_view(
            self.client_tuning.as_deref(),
            &self.client_environment_volumes,
        )
    }

    /// Seed the host and the client with independent level gravity, as two peers
    /// whose `initialGravity` (or a pre-join `setGravity`) differ.
    pub(crate) fn with_level_gravity(
        link: LinkConfig,
        host_level_gravity: f32,
        client_level_gravity: f32,
    ) -> Self {
        let mut h = Self::new(link);
        h.host_level_gravity = host_level_gravity;
        h.client_level_gravity = client_level_gravity;
        h
    }

    /// The level gravity the client's installed host tuning carries, if any.
    pub(crate) fn client_installed_level_gravity(&self) -> Option<f32> {
        self.client_tuning
            .as_deref()
            .map(|payload| payload.environment.level_gravity)
    }

    pub(crate) fn with_moving_platform(link: LinkConfig) -> Self {
        Self::with_platform_initial_phase(link, false)
    }

    /// Shared authored motion for the steady prediction/drift gate. Host-only
    /// trigger transitions use the stopped `with_moving_platform` fixture instead.
    pub(crate) fn with_started_moving_platform(link: LinkConfig) -> Self {
        Self::with_platform_initial_phase(link, true)
    }

    fn with_platform_initial_phase(link: LinkConfig, started: bool) -> Self {
        let mut h = Self::new(link);
        h.world = CollisionWorld::new();

        let host_mover = spawn_platform_mover(&mut h.host_registry, started);
        let client_mover = spawn_platform_mover(&mut h.client_registry, started);
        h.host_registry
            .set_tags(host_mover, vec!["harness-platform".into()])
            .expect("tag host mover");
        spawn_harness_trigger(
            &mut h.host_registry,
            &mut h.host_trigger_bridge,
            TriggerActivation::Touch,
            TriggerCommand::Start,
        );
        h.host_use_trigger = Some(spawn_harness_trigger(
            &mut h.host_registry,
            &mut h.host_trigger_bridge,
            TriggerActivation::Use,
            TriggerCommand::GoToPathNode("finish".into()),
        ));
        h.mover_colliders
            .push(platform_collider(MOVING_PLATFORM_ID));
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.allocator.stamp(host_mover);
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// A started mover carrying a ledge: a riser facing +Z at `riser_z`, its top
    /// `top_y` above the static floor, sliding along X at the platform speed
    /// across the pawn's forward path. Static floor beneath for the approach.
    pub(crate) fn with_moving_ledge(link: LinkConfig, riser_z: f32, top_y: f32) -> Self {
        let mut h = Self::new(link);
        let origin = Vec3::new(0.0, 0.0, riser_z);
        let waypoints = vec![origin, origin + Vec3::new(6.0, 0.0, 0.0)];
        let host_mover = spawn_path_mover(&mut h.host_registry, waypoints.clone());
        let client_mover = spawn_path_mover(&mut h.client_registry, waypoints);
        h.mover_colliders
            .push(ledge_collider(MOVING_PLATFORM_ID, top_y));
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.allocator.stamp(host_mover);
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// An unstarted default (`displace`) `once` lift with `collider`, travelling
    /// `waypoints` at `speed_mps`, in `world` on both peers. The pawn spawns at
    /// [`START`], over the lift's origin.
    pub(crate) fn with_lift(
        link: LinkConfig,
        world: CollisionWorld,
        collider: MoverCollider,
        waypoints: Vec<Vec3>,
        speed_mps: f32,
    ) -> Self {
        let mut h = Self::new(link);
        h.world = world;
        let spawn = |registry: &mut EntityRegistry| {
            spawn_mover(
                registry,
                collider.mover_id(),
                waypoints.clone(),
                speed_mps,
                KinematicMoverMode::Once,
                false,
            )
        };
        let host_mover = spawn(&mut h.host_registry);
        let client_mover = spawn(&mut h.client_registry);
        h.mover_colliders.push(collider);
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.allocator.stamp(host_mover);
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// A pure rotator with a rider planted one metre from its pivot. The static
    /// axis/acceleration remain local on both peers; only the phase replicates.
    pub(crate) fn with_rotating_platform(link: LinkConfig) -> Self {
        let mut h = Self::new(link);
        h.world = CollisionWorld::new();
        h.host_registry
            .set_component(
                h.host_pawn,
                Transform {
                    position: ROTATING_PLATFORM_RIDER_START,
                    ..Transform::default()
                },
            )
            .expect("place rider on rotating platform");

        let host_mover = spawn_rotating_platform_mover(&mut h.host_registry);
        let client_mover = spawn_rotating_platform_mover(&mut h.client_registry);
        h.mover_colliders
            .push(platform_collider(MOVING_PLATFORM_ID));
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.allocator.stamp(host_mover);
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }
}

fn spawn_platform_mover(registry: &mut EntityRegistry, started: bool) -> EntityId {
    let id = registry.spawn(Transform::default());
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0)],
            waypoint_names: vec!["start".to_string(), "finish".to_string()],
            speed_mps: MOVING_PLATFORM_SPEED_MPS,
            wait_ms: 0.0,
            mode: KinematicMoverMode::PingPong,
            started,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

fn spawn_path_mover(registry: &mut EntityRegistry, waypoints: Vec<Vec3>) -> EntityId {
    let id = registry.spawn(Transform {
        position: waypoints[0],
        ..Transform::default()
    });
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoint_names: (0..waypoints.len()).map(|i| format!("node{i}")).collect(),
            waypoints,
            speed_mps: MOVING_PLATFORM_SPEED_MPS,
            wait_ms: 0.0,
            mode: KinematicMoverMode::PingPong,
            started: true,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

fn spawn_mover(
    registry: &mut EntityRegistry,
    mover_id: u32,
    waypoints: Vec<Vec3>,
    speed_mps: f32,
    mode: KinematicMoverMode,
    started: bool,
) -> EntityId {
    let id = registry.spawn(Transform {
        position: waypoints[0],
        ..Transform::default()
    });
    let mover = KinematicMoverComponent::new(
        mover_id,
        postretro_entities::KinematicMoverConfig {
            waypoint_names: (0..waypoints.len()).map(|i| format!("node{i}")).collect(),
            waypoints,
            speed_mps,
            wait_ms: 0.0,
            mode,
            started,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

fn spawn_rotating_platform_mover(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO],
            waypoint_names: vec!["center".to_string()],
            speed_mps: 0.0,
            wait_ms: 0.0,
            mode: KinematicMoverMode::Once,
            started: true,
            spin_axis: Vec3::Y,
            initial_spin_rate_rad_s: ROTATING_PLATFORM_INITIAL_RATE_RAD_S,
            spin_accel_rad_s2: ROTATING_PLATFORM_ACCEL_RAD_S2,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

/// Host-only trigger fixture for the network harness. Both volumes contain the
/// remote pawn's spawn capsule: the touch trigger proves trigger evaluation is a
/// host tick responsibility, while the use trigger exercises the client-input
/// `use_pressed -> PlayerId::Remote` seam.
fn spawn_harness_trigger(
    registry: &mut EntityRegistry,
    bridge: &mut TriggerVolumeBridge,
    activation: TriggerActivation,
    command: TriggerCommand,
) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            TriggerVolumeComponent::new(
                activation,
                "harness-platform".into(),
                String::new(),
                String::new(),
                command,
                TriggerFireMode::Once,
                0.0,
                true,
            ),
        )
        .expect("attach harness trigger");
    bridge.insert_for_test(id, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.5, 1.0));
    id
}

/// A mover-local ledge: a riser facing +Z at local z = 0 up to `top_y`, and its
/// top running to local z = -6, 40 m wide.
fn ledge_collider(mover_id: u32, top_y: f32) -> MoverCollider {
    MoverCollider::from_local_triangles(
        mover_id,
        &[
            Vec3::new(-20.0, 0.0, 0.0),
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(20.0, top_y, 0.0),
            Vec3::new(-20.0, top_y, 0.0),
            Vec3::new(-20.0, top_y, -6.0),
            Vec3::new(20.0, top_y, -6.0),
            Vec3::new(20.0, top_y, 0.0),
            Vec3::new(-20.0, top_y, 0.0),
        ],
        &[[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6]],
    )
    .unwrap()
}

fn platform_collider(mover_id: u32) -> MoverCollider {
    MoverCollider::from_local_triangles(
        mover_id,
        &[
            Vec3::new(-1.75, 0.0, -1.75),
            Vec3::new(1.75, 0.0, -1.75),
            Vec3::new(1.75, 0.0, 1.75),
            Vec3::new(-1.75, 0.0, 1.75),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()
}
