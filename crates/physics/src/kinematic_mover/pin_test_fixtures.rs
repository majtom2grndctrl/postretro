// Host tick-order rig for mover pin tests: driver, player movement and the
// blocking pass in `simulate_tick`'s order, over box geometry.
// See: context/lib/movement.md §6 · context/lib/entity_model.md §5

use std::collections::HashMap;

use glam::{Vec2, Vec3};
use parry3d::shape::Capsule;
use postretro_collision::EnvironmentView;
use postretro_entities::{
    BlockPolicy, ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverConfig, KinematicMoverMode, MoverCommand, Transform,
};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams, GroundRef,
    PlayerMovementComponent, PlayerMovementDescriptor, SpeedParams,
};

use super::{
    MoverAutoCloseTimers, MoverEventKind, MoverTickStateTable, apply_mover_command,
    run_kinematic_mover_tick, run_mover_blocking_pass, run_mover_pin_pretest,
};
use crate::collision::moving::{
    CombinedCollisionWorld, MoverCollider, deepest_mover_penetration, point_inside_mover,
};
use crate::collision::{CollisionCapsule, CollisionWorld, SKIN_DISTANCE};
use crate::movement::{MovementInput, tick};

pub(super) const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);

/// Standing capsule: 0.4 m radius, 0.8 m half height, so 2.4 m tall.
pub(super) const RADIUS: f32 = 0.4;
pub(super) const HALF_HEIGHT: f32 = 0.8;

pub(super) fn descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
        sounds: None,
        capsule: CapsuleParams {
            radius: RADIUS,
            half_height: HALF_HEIGHT,
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
        knockback: Default::default(),
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        slide: None,
        swim: None,
        vault: None,
        view_feel: None,
    }
}

/// Centre height of a standing pawn resting on a floor at `floor_y`.
pub(super) fn rest_y(floor_y: f32) -> f32 {
    floor_y + HALF_HEIGHT + RADIUS + SKIN_DISTANCE
}

pub(super) fn idle() -> MovementInput {
    MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        facing_pitch: 0.0,
        use_pressed: false,
        drop_pressed: false,
    }
}

/// Walk along world `direction` (horizontal). At yaw 0, input x is world +X and
/// input y (forward) is world -Z.
pub(super) fn walk(direction: Vec3) -> MovementInput {
    MovementInput {
        wish_dir: Vec2::new(direction.x, -direction.z).normalize_or_zero(),
        ..idle()
    }
}

/// Closed axis-aligned box triangles, wound outward.
pub(super) fn box_triangles(
    points: &mut Vec<Vec3>,
    tris: &mut Vec<[u32; 3]>,
    min: Vec3,
    max: Vec3,
) {
    let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let faces = [
        (
            [
                c(min.x, min.y, min.z),
                c(min.x, max.y, min.z),
                c(min.x, max.y, max.z),
                c(min.x, min.y, max.z),
            ],
            Vec3::NEG_X,
        ),
        (
            [
                c(max.x, min.y, min.z),
                c(max.x, max.y, min.z),
                c(max.x, max.y, max.z),
                c(max.x, min.y, max.z),
            ],
            Vec3::X,
        ),
        (
            [
                c(min.x, min.y, min.z),
                c(max.x, min.y, min.z),
                c(max.x, min.y, max.z),
                c(min.x, min.y, max.z),
            ],
            Vec3::NEG_Y,
        ),
        (
            [
                c(min.x, max.y, min.z),
                c(max.x, max.y, min.z),
                c(max.x, max.y, max.z),
                c(min.x, max.y, max.z),
            ],
            Vec3::Y,
        ),
        (
            [
                c(min.x, min.y, min.z),
                c(max.x, min.y, min.z),
                c(max.x, max.y, min.z),
                c(min.x, max.y, min.z),
            ],
            Vec3::NEG_Z,
        ),
        (
            [
                c(min.x, min.y, max.z),
                c(max.x, min.y, max.z),
                c(max.x, max.y, max.z),
                c(min.x, max.y, max.z),
            ],
            Vec3::Z,
        ),
    ];
    for (corners, normal) in faces {
        let base = points.len() as u32;
        points.extend(corners);
        let face = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        if face.dot(normal) >= 0.0 {
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
        } else {
            tris.push([base, base + 2, base + 1]);
            tris.push([base, base + 3, base + 2]);
        }
    }
}

/// Static world from boxes, each `(min, max)`.
pub(super) fn static_boxes(boxes: &[(Vec3, Vec3)]) -> CollisionWorld {
    let (mut points, mut tris) = (Vec::new(), Vec::new());
    for (min, max) in boxes {
        box_triangles(&mut points, &mut tris, *min, *max);
    }
    CollisionWorld::from_triangles_for_test(points, tris)
}

/// The floor every fixture stands on: top at y = 0.
pub(super) fn floor() -> (Vec3, Vec3) {
    (Vec3::new(-30.0, -1.0, -30.0), Vec3::new(30.0, 0.0, 30.0))
}

/// Mover collider from mover-local boxes.
pub(super) fn box_collider(mover_id: u32, boxes: &[(Vec3, Vec3)]) -> MoverCollider {
    let (mut points, mut tris) = (Vec::new(), Vec::new());
    for (min, max) in boxes {
        box_triangles(&mut points, &mut tris, *min, *max);
    }
    MoverCollider::from_local_triangles(mover_id, &points, &tris).expect("box collider is valid")
}

/// A lift deck: `half` wide in X and Z, 0.2 m thick, its top at local y = 0.
pub(super) fn deck(mover_id: u32, half: f32) -> MoverCollider {
    box_collider(
        mover_id,
        &[(Vec3::new(-half, -0.2, -half), Vec3::new(half, 0.0, half))],
    )
}

/// What one rig tick produced.
#[derive(Debug, Default, Clone)]
pub(super) struct TickReport {
    pub events: Vec<(MoverEventKind, u32)>,
    pub impacts: usize,
}

impl TickReport {
    pub(super) fn blocked_events(&self, mover_id: u32) -> usize {
        self.events
            .iter()
            .filter(|(kind, id)| *kind == MoverEventKind::Blocked && *id == mover_id)
            .count()
    }
}

/// One host fixed tick in `simulate_tick` order over a small registry.
pub(super) struct Rig {
    pub registry: EntityRegistry,
    pub world: CollisionWorld,
    pub colliders: Vec<MoverCollider>,
    pub table: MoverTickStateTable,
    pub auto_close: Option<MoverAutoCloseTimers>,
    inputs: HashMap<EntityId, MovementInput>,
    pawns: Vec<EntityId>,
    desc: PlayerMovementDescriptor,
}

impl Rig {
    pub(super) fn new(world: CollisionWorld) -> Self {
        Self {
            registry: EntityRegistry::new(),
            world,
            colliders: Vec::new(),
            table: MoverTickStateTable::default(),
            auto_close: None,
            inputs: HashMap::new(),
            pawns: Vec::new(),
            desc: descriptor(),
        }
    }

    /// Add an unstarted mover travelling `waypoints` (world positions of its
    /// origin) at `speed` m/s, with the given collider.
    pub(super) fn add_mover(
        &mut self,
        collider: MoverCollider,
        waypoints: Vec<Vec3>,
        speed_mps: f32,
        mode: KinematicMoverMode,
        policy: BlockPolicy,
    ) -> EntityId {
        let mover_id = collider.mover_id;
        let names = (0..waypoints.len()).map(|i| format!("node{i}")).collect();
        let mut mover = KinematicMoverComponent::new(
            mover_id,
            KinematicMoverConfig {
                waypoints: waypoints.clone(),
                waypoint_names: names,
                speed_mps,
                wait_ms: 0.0,
                mode,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        mover.block_policy = policy;
        let entity = self.registry.spawn(Transform {
            position: waypoints[0],
            ..Transform::default()
        });
        self.registry
            .set_component(entity, mover)
            .expect("mover attaches");
        self.colliders.push(collider);
        entity
    }

    /// Add a pawn standing at `position` (capsule centre) and settle it with
    /// every mover still unstarted.
    pub(super) fn add_pawn(&mut self, position: Vec3) -> EntityId {
        let pawn = self.registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        self.registry
            .set_component(pawn, PlayerMovementComponent::from_descriptor(&self.desc))
            .expect("pawn movement attaches");
        self.pawns.push(pawn);
        self.inputs.insert(pawn, idle());
        pawn
    }

    pub(super) fn remove_pawn(&mut self, pawn: EntityId) {
        self.registry.despawn(pawn).expect("pawn despawns");
        self.pawns.retain(|id| *id != pawn);
        self.inputs.remove(&pawn);
    }

    /// Visit pawns in reverse spawn order from now on.
    pub(super) fn reverse_pawn_order(&mut self) {
        self.pawns.reverse();
    }

    pub(super) fn set_input(&mut self, pawn: EntityId, input: MovementInput) {
        self.inputs.insert(pawn, input);
    }

    pub(super) fn mover(&self, entity: EntityId) -> KinematicMoverComponent {
        self.registry
            .get_component::<KinematicMoverComponent>(entity)
            .expect("mover is live")
            .clone()
    }

    pub(super) fn edit_mover(
        &mut self,
        entity: EntityId,
        edit: impl FnOnce(&mut KinematicMoverComponent),
    ) {
        let Ok(ComponentValue::KinematicMover(mover)) = self
            .registry
            .get_component_value_mut(entity, ComponentKind::KinematicMover)
        else {
            panic!("mover is live");
        };
        edit(mover);
    }

    /// Apply a mover command between ticks, as a trigger, the frame-end
    /// script drain or a player event's fire list does.
    pub(super) fn command(&mut self, entity: EntityId, command: MoverCommand) {
        self.edit_mover(entity, |mover| apply_mover_command(mover, &command));
    }

    pub(super) fn start(&mut self, entity: EntityId) {
        self.command(entity, MoverCommand::Start);
    }

    pub(super) fn mover_position(&self, entity: EntityId) -> Vec3 {
        self.registry
            .get_component::<Transform>(entity)
            .expect("mover transform")
            .position
    }

    pub(super) fn pawn(&self, pawn: EntityId) -> PlayerMovementComponent {
        self.registry
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("pawn is live")
            .clone()
    }

    pub(super) fn edit_pawn(
        &mut self,
        pawn: EntityId,
        edit: impl FnOnce(&mut PlayerMovementComponent),
    ) {
        let mut component = self.pawn(pawn);
        edit(&mut component);
        self.registry
            .set_component(pawn, component)
            .expect("pawn updates");
    }

    pub(super) fn position(&self, pawn: EntityId) -> Vec3 {
        self.registry
            .get_component::<Transform>(pawn)
            .expect("pawn transform")
            .position
    }

    pub(super) fn set_position(&mut self, pawn: EntityId, position: Vec3) {
        let mut transform = *self
            .registry
            .get_component::<Transform>(pawn)
            .expect("pawn transform");
        transform.position = position;
        self.registry
            .set_component(pawn, transform)
            .expect("pawn transform updates");
    }

    fn capsule(&self, pawn: EntityId) -> CapsuleParams {
        self.pawn(pawn).capsule
    }

    /// Depth the pawn's capsule overlaps static geometry.
    pub(super) fn static_overlap(&self, pawn: EntityId) -> f32 {
        let capsule = self.capsule(pawn);
        self.world.overlap_depth_for_test(
            self.position(pawn),
            CollisionCapsule::new(capsule.radius, capsule.half_height),
        )
    }

    /// Depth the pawn's capsule overlaps any mover at its published pose,
    /// beyond touching.
    pub(super) fn mover_overlap(&self, pawn: EntityId) -> f32 {
        let capsule = self.capsule(pawn);
        let shape = Capsule::new_y(capsule.half_height, capsule.radius);
        deepest_mover_penetration(&self.colliders, &self.table, self.position(pawn), &shape)
            .map_or(0.0, |penetration| {
                (penetration.depth - SKIN_DISTANCE).max(0.0)
            })
    }

    /// Whether the pawn's centre lies inside any mover's closed hull.
    pub(super) fn inside_mover_hull(&self, pawn: EntityId) -> bool {
        point_inside_mover(&self.colliders, &self.table, self.position(pawn))
    }

    /// Run idle ticks until every pawn has settled.
    pub(super) fn settle(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.tick();
        }
    }

    /// Pawns added from now on use `desc`.
    pub(super) fn use_descriptor(&mut self, desc: PlayerMovementDescriptor) {
        self.desc = desc;
    }

    /// One host fixed tick.
    pub(super) fn tick(&mut self) -> TickReport {
        self.tick_with(|_| {})
    }

    /// One host fixed tick, running `trigger_stage` after player movement and
    /// before auto-close and the blocking pass, where the trigger tick's
    /// commands land.
    pub(super) fn tick_with(&mut self, trigger_stage: impl FnOnce(&mut Rig)) -> TickReport {
        let mut events = Vec::new();
        let mut impacts = 0usize;

        {
            let (poses, blocking_state) = self.table.split_for_blocking();
            run_mover_pin_pretest(
                &mut self.registry,
                &self.world,
                &self.colliders,
                &poses,
                blocking_state,
                DT,
                &mut events,
            );
        }
        run_kinematic_mover_tick(&mut self.registry, &mut self.table, DT);
        events.extend(self.table.terminus_events());
        if let Some(auto_close) = self.auto_close.as_ref() {
            auto_close.arm_opened_termini(&mut self.registry, &events);
        }

        let collision = CombinedCollisionWorld::new(&self.world, &self.colliders, &self.table);
        let snapshots: Vec<_> = self
            .pawns
            .iter()
            .map(|pawn| {
                (
                    *pawn,
                    self.pawn(*pawn),
                    self.position(*pawn),
                    self.inputs.get(pawn).cloned().unwrap_or_else(idle),
                )
            })
            .collect();
        let mut advanced = Vec::with_capacity(snapshots.len());
        for (pawn, mut component, position, input) in snapshots {
            let (next, _events) = tick(&mut component, &input, &collision, ENV, DT, position);
            advanced.push((pawn, component, next));
        }
        for (pawn, component, next) in advanced {
            self.set_position(pawn, next);
            self.registry
                .set_component(pawn, component)
                .expect("pawn updates");
        }

        trigger_stage(self);
        if let Some(auto_close) = self.auto_close.as_ref() {
            auto_close.tick(&mut self.registry, DT);
        }
        let (poses, blocking_state) = self.table.split_for_blocking();
        run_mover_blocking_pass(
            &mut self.registry,
            &self.world,
            &self.colliders,
            &poses,
            blocking_state,
            DT,
            &mut events,
            &mut |_| impacts += 1,
        );
        TickReport { events, impacts }
    }

    /// Ground the pawn on `mover_id`, as a rider that settled on it would be.
    pub(super) fn assert_rides(&self, pawn: EntityId, mover_id: u32) {
        assert_eq!(
            self.pawn(pawn).ground,
            GroundRef::Mover(mover_id),
            "the fixture pawn rides mover {mover_id} at {:?}",
            self.position(pawn)
        );
    }
}
