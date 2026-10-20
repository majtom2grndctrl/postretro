// Host-side multi-pawn movement seam (M15 Phase 3 Task 4). Advances an explicit
// set of `(EntityId, MovementInput)` pairs through `movement::tick`, writing each
// resulting `Transform` + `PlayerMovementComponent` (and the output-only
// `ImmersionComponent`) back to the registry and retaining per-pawn movement
// events for the caller to scope.
// See: context/lib/networking.md · context/lib/movement.md
//
// This is the authoritative-host counterpart to `simulate_tick`'s single-pawn
// `run_movement_tick`. It NEVER consults `local_movement_pawn()` or the single
// local-player marker: every authoritative movement pawn — including the listen
// host's own player pawn — is named explicitly in the input list. The host fixed
// tick runs THIS first, then AI/agent/weapon/death exactly as `simulate_tick` does.
//
// Boundary: registry-driven (it reads/writes Transform + PlayerMovementComponent),
// but it adds no networking branch to `movement::tick` — the per-pawn input is
// already resolved by the netcode command queue before this seam runs.

use glam::{Quat, Vec3};

use crate::movement::{
    MovementCollisionSource, MovementEvents, MovementInput, tick_with_immersion_and_armed,
};
use postretro_collision::EnvironmentView;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{ImmersionComponent, PlayerMovementComponent};

/// Advance the named movement pawns one fixed tick. Snapshots each pawn's component
/// and position ONCE at the start of the tick, runs `movement::tick` for each, writes
/// the resulting `Transform` plus movement component back, stores the tick's resolved
/// immersion in the pawn's output-only `ImmersionComponent` (the `player.immersion`
/// slot's source; this authoritative seam is its only writer), and returns one
/// pawn-identified event record per successfully advanced pawn in input order. The
/// caller owns event scope: host gameplay selects only its local pawn, while client
/// prediction advances exactly one local pawn. Reading all pawns before writing any
/// keeps each pawn's input applied to its start-of-tick state, never to another
/// pawn's mid-tick write.
///
/// Precondition: `pawn_inputs` must name each `EntityId` at most once. The seam pushes
/// one snapshot per occurrence, so a duplicated id would double-apply its input and
/// double-count its events — the snapshot discipline orders reads-before-writes but
/// does NOT dedup. Today's sole caller resolves inputs from a per-pawn `HashMap`, so
/// uniqueness holds by construction; this is a documented caller contract, not a guard
/// enforced here.
///
/// Pawns whose `EntityId` lacks a live `PlayerMovementComponent` or `Transform` are
/// skipped (a stale id from a despawn racing the command queue); they contribute no
/// events and no write. The caller is responsible for ordering: the host runs this
/// BEFORE AI/agent/weapon/death, matching `simulate_tick`'s movement-first order.
pub fn run_host_movement_tick(
    registry: &mut EntityRegistry,
    collision: &impl MovementCollisionSource,
    environment: EnvironmentView<'_>,
    pawn_inputs: &[(EntityId, MovementInput)],
    tick_dt: f32,
) -> Vec<(EntityId, MovementEvents)> {
    // Snapshot every pawn's mutable movement state + position up front. Reading all
    // pawns before writing any keeps the tick's reads consistent (a pawn's input is
    // applied to its start-of-tick state, never to another pawn's mid-tick write) and
    // mirrors `simulate_tick`'s snapshot-then-advance discipline.
    let mut snapshots: Vec<(EntityId, PlayerMovementComponent, Vec3, MovementInput)> =
        Vec::with_capacity(pawn_inputs.len());
    for (id, input) in pawn_inputs {
        if let (Ok(component), Ok(transform)) = (
            registry.get_component::<PlayerMovementComponent>(*id),
            registry.get_component::<Transform>(*id),
        ) {
            let mut component = component.clone();
            drop_vault_if_dead(registry, *id, &mut component);
            snapshots.push((*id, component, transform.position, input.clone()));
        }
    }

    let mut events_out = Vec::with_capacity(snapshots.len());
    for (id, mut component, position, input) in snapshots {
        let world = collision.combined_collision();
        let (new_pos, events, immersion) = tick_with_immersion_and_armed(
            &mut component,
            &input,
            collision,
            environment,
            tick_dt,
            position,
            |index| world.static_world.teleporters.armed(registry, index),
        );
        if let Some(overlap) = &events.static_overlap {
            super::report_static_overlap(world.static_world, registry, id, overlap);
        }
        // Write the advanced pose. `get_component` re-read tolerates a Transform that
        // vanished mid-loop (it cannot here — single-threaded — but keeps the write
        // total).
        if let Ok(transform) = registry.get_component::<Transform>(id) {
            let mut t = *transform;
            t.position = new_pos;
            // Transform yaw is the authoritative aim direction. Lower-body travel
            // heading is a render-only override derived from velocity, so snapshots
            // retain the controlling camera yaw for upper-body presentation.
            if input.facing_yaw.is_finite() {
                t.rotation = Quat::from_rotation_y(
                    input.facing_yaw + events.teleport.map_or(0.0, |event| event.yaw_delta),
                );
            }
            if events.teleport.is_some() {
                let _ = registry.set_presentation_transform(id, t);
            } else {
                let _ = registry.set_component(id, t);
            }
        }
        let _ = registry.set_component(id, component);
        let _ = registry.set_component(id, ImmersionComponent { immersion });
        events_out.push((id, events));
    }

    events_out
}

/// Settle vault state on `pawn`'s liveness before its movement tick. Movement
/// itself never reads health; host movement and client prediction both call
/// this, so the two take the same path.
///
/// A pawn dead or committed to removal leaves `Vault` through
/// `PlayerMovementComponent::break_off_vault`: its path velocity is dropped, so
/// it falls off the ledge path under gravity instead of finishing the climb or
/// being thrown on by the rise. It is also barred from starting a vault this
/// tick (`vault_barred`), or a dead pawn pressing into a ledge would re-enter
/// `Vault` on the same tick, every tick. No state edge is emitted: death owns
/// the moment, not `vault_ended`.
pub fn drop_vault_if_dead(
    registry: &EntityRegistry,
    pawn: EntityId,
    movement: &mut PlayerMovementComponent,
) {
    let dead = crate::scripting_systems::health::is_quiescent(registry, pawn);
    movement.vault_barred = dead;
    if dead {
        movement.break_off_vault();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionWorld;
    use glam::Vec2;
    use glam::Vec3;

    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, MovementState,
        PlayerMovementDescriptor, SpeedParams,
    };

    const EPSILON: f32 = 1e-4;
    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: f32 = -20.0;

    fn floor_world() -> CollisionWorld {
        let points = vec![
            Vec3::new(-500.0, 0.0, -500.0),
            Vec3::new(500.0, 0.0, -500.0),
            Vec3::new(500.0, 0.0, 500.0),
            Vec3::new(-500.0, 0.0, 500.0),
        ];
        let triangles = vec![[0, 2, 1], [0, 3, 2]];
        CollisionWorld::from_triangles_for_test(points, triangles)
    }

    fn descriptor() -> PlayerMovementDescriptor {
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
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            swim: None,
            vault: None,
            view_feel: None,
        }
    }

    /// Spawn a movement pawn at `pos` carrying a fresh descriptor-derived component.
    fn spawn_pawn(registry: &mut EntityRegistry, pos: Vec3) -> EntityId {
        let id = registry.spawn(Transform {
            position: pos,
            ..Transform::default()
        });
        let component = PlayerMovementComponent::from_descriptor(&descriptor());
        registry.set_component(id, component).unwrap();
        id
    }

    fn forward_input() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::new(0.0, 1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        }
    }

    fn idle_input() -> MovementInput {
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

    fn vaulting(pawn: EntityId, registry: &mut EntityRegistry) {
        let mut component = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        component.movement_state = MovementState::Vault {
            elapsed_ms: 32.0,
            target: Vec3::new(1.0, 2.0, 0.0),
            start_offset: Vec3::new(-0.8, -0.7, 0.0),
            mover: None,
            entry_speed: 4.0,
        };
        registry.set_component(pawn, component).unwrap();
    }

    fn set_health(registry: &mut EntityRegistry, pawn: EntityId, current: f32) {
        registry
            .set_component(
                pawn,
                postretro_entities::components::health::HealthComponent {
                    max: 100.0,
                    current,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: std::collections::HashMap::new(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
    }

    // A pawn that dies mid-vault drops off the ledge path; a live one keeps
    // climbing. Movement never reads health, so the host tick checks first.
    #[test]
    fn a_dead_pawn_leaves_its_vault_before_its_movement_tick() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let pawn = spawn_pawn(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        vaulting(pawn, &mut registry);
        set_health(&mut registry, pawn, 50.0);
        let mut live = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        drop_vault_if_dead(&registry, pawn, &mut live);
        assert!(
            matches!(live.movement_state, MovementState::Vault { .. }),
            "a live pawn keeps vaulting"
        );

        set_health(&mut registry, pawn, 0.0);
        run_host_movement_tick(
            &mut registry,
            &world,
            EnvironmentView::level_only(GRAVITY),
            &[(pawn, idle_input())],
            DT,
        );
        let dead = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        assert_eq!(
            dead.movement_state,
            MovementState::Normal,
            "a dead pawn leaves the vault"
        );
    }

    // Regression: death mid-vault kept the rise velocity and a swim-entered
    // vault's re-entry latch, throwing the pawn on and, in deep water, never
    // letting it swim again.
    #[test]
    fn a_dead_pawn_drops_its_vault_velocity_and_swim_latch() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_pawn(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        vaulting(pawn, &mut registry);
        set_health(&mut registry, pawn, 0.0);
        let mut live = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        let knockback = Vec3::new(0.0, 0.0, 2.0);
        live.knockback_velocity = knockback;
        live.velocity = Vec3::new(1.0, 9.0, 0.0) + knockback;
        live.swim_reentry_latched = true;

        drop_vault_if_dead(&registry, pawn, &mut live);
        assert_eq!(live.movement_state, MovementState::Normal);
        assert_eq!(live.velocity, knockback, "only the knockback stays");
        assert!(!live.swim_reentry_latched);
        assert!(live.vault_barred, "a dead pawn may not start a vault");

        set_health(&mut registry, pawn, 50.0);
        drop_vault_if_dead(&registry, pawn, &mut live);
        assert!(!live.vault_barred, "a live pawn may vault again");
    }

    /// Floor at y = 0 for z > -1, a riser facing +Z at z = -1, and a top at
    /// y = 1.2 beyond it. Facing 0 looks down -Z, into the riser.
    fn ledge_world() -> CollisionWorld {
        let points = vec![
            Vec3::new(-50.0, 0.0, -1.0),
            Vec3::new(50.0, 0.0, -1.0),
            Vec3::new(50.0, 0.0, 50.0),
            Vec3::new(-50.0, 0.0, 50.0),
            Vec3::new(-50.0, 1.2, -1.0),
            Vec3::new(50.0, 1.2, -1.0),
            Vec3::new(50.0, 1.2, -50.0),
            Vec3::new(-50.0, 1.2, -50.0),
        ];
        let triangles = vec![
            // Floor, facing up.
            [0, 2, 1],
            [0, 3, 2],
            // Riser, facing +Z.
            [0, 1, 5],
            [0, 5, 4],
            // Top, facing up.
            [4, 5, 6],
            [4, 6, 7],
        ];
        CollisionWorld::from_triangles_for_test(points, triangles)
    }

    // Regression: the death drop ran before the pawn's tick, but the same tick's
    // vault entry had no liveness check, so a dead pawn pressing into a ledge
    // re-entered `Vault` and re-fired `vault_started` every tick.
    #[test]
    fn a_dead_pawn_pressing_into_a_ledge_never_enters_vault() {
        let world = ledge_world();
        let mut desc = descriptor();
        desc.vault = Some(postretro_foundation::VaultParams {
            max_height: 1.6,
            swim_max_height: 0.9,
            duration_ms: 320.0,
            from: postretro_foundation::VaultEntry::default(),
            allow_fire: false,
        });
        // Airborne, feet 0.5 m up, capsule surface 5 cm short of the riser,
        // moving into it.
        let start = Vec3::new(0.0, 0.5 + 1.2, -1.0 + 0.4 + 0.05);
        for (health, vaults) in [(100.0, true), (0.0, false)] {
            let mut registry = EntityRegistry::new();
            let pawn = registry.spawn(Transform {
                position: start,
                ..Transform::default()
            });
            let mut component = PlayerMovementComponent::from_descriptor(&desc);
            component.velocity = Vec3::new(0.0, 0.0, -7.0);
            registry.set_component(pawn, component).unwrap();
            set_health(&mut registry, pawn, health);

            let mut entered = false;
            for _ in 0..10 {
                let events = run_host_movement_tick(
                    &mut registry,
                    &world,
                    EnvironmentView::level_only(GRAVITY),
                    &[(pawn, forward_input())],
                    DT,
                );
                entered |= events.iter().any(|(_, event)| {
                    event
                        .state_edges
                        .iter()
                        .any(|edge| edge.to == postretro_foundation::MovementStateKind::Vault)
                });
            }
            assert_eq!(entered, vaults, "health {health}");
        }
    }

    // The seam advances ONLY the explicit pawns and writes both Transform and
    // PlayerMovementComponent back. A pawn not in the list is untouched even though
    // it carries PlayerMovement — proving the seam never falls back to
    // local_movement_pawn / a single marker.
    #[test]
    fn advances_only_explicit_pawns_and_writes_back_both_components() {
        let mut registry = EntityRegistry::new();
        let world = floor_world();
        let driven = spawn_pawn(&mut registry, Vec3::new(0.0, 1.21, 0.0));
        let untouched = spawn_pawn(&mut registry, Vec3::new(10.0, 1.21, 0.0));

        let before_untouched = registry
            .get_component::<Transform>(untouched)
            .unwrap()
            .position;

        // First tick: settle grounded. A pawn starts Airborne by design
        // (`PlayerMovementComponent::from_descriptor`), and a first-tick
        // forward input only gets air control: 9.7e-5 of travel, under
        // `EPSILON`.
        let _ = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(driven, idle_input())],
            DT,
        );
        let inputs = vec![(driven, forward_input())];
        let _ = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &inputs,
            DT,
        );

        // The driven pawn moved along -Z (facing 0 looks down -Z) and its movement
        // component reflects a grounded tick (Transform + component both written).
        let driven_t = registry.get_component::<Transform>(driven).unwrap();
        assert!(
            driven_t.position.z < -EPSILON,
            "driven pawn advanced forward; z={}",
            driven_t.position.z
        );
        let driven_m = registry
            .get_component::<PlayerMovementComponent>(driven)
            .unwrap();
        assert!(driven_m.is_grounded(), "floored pawn grounded after a tick");

        // The pawn NOT in the input list is untouched — the seam ignores it entirely.
        let after_untouched = registry
            .get_component::<Transform>(untouched)
            .unwrap()
            .position;
        assert!(
            (after_untouched - before_untouched).length() < EPSILON,
            "a pawn absent from the input list is never moved"
        );
    }

    #[test]
    fn movement_transform_yaw_tracks_aim_while_stationary() {
        let mut registry = EntityRegistry::new();
        let world = floor_world();
        let pawn = spawn_pawn(&mut registry, Vec3::new(0.0, 1.21, 0.0));
        let mut input = idle_input();
        input.facing_yaw = 0.8;

        let _ = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(pawn, input)],
            DT,
        );

        let transform = registry.get_component::<Transform>(pawn).unwrap();
        let (yaw, _, _) = transform.rotation.to_euler(glam::EulerRot::YXZ);
        assert!((yaw - 0.8).abs() <= EPSILON);
    }

    // Multiple pawns advance independently in one tick, each by its own input.
    #[test]
    fn advances_each_pawn_by_its_own_input() {
        let mut registry = EntityRegistry::new();
        let world = floor_world();
        let mover = spawn_pawn(&mut registry, Vec3::new(0.0, 1.21, 0.0));
        let idler = spawn_pawn(&mut registry, Vec3::new(5.0, 1.21, 0.0));
        // First tick: settle both grounded. Pawns start Airborne by design
        // (`PlayerMovementComponent::from_descriptor`), and a first-tick
        // forward input only gets air control: 9.7e-5 of travel, under
        // `EPSILON`.
        let settle = vec![(mover, idle_input()), (idler, idle_input())];
        let _ = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &settle,
            DT,
        );

        let mover_before = registry.get_component::<Transform>(mover).unwrap().position;
        let idler_before = registry.get_component::<Transform>(idler).unwrap().position;

        let inputs = vec![(mover, forward_input()), (idler, idle_input())];
        let _ = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &inputs,
            DT,
        );

        let mover_after = registry.get_component::<Transform>(mover).unwrap().position;
        let idler_after = registry.get_component::<Transform>(idler).unwrap().position;

        assert!(
            (mover_after.z - mover_before.z).abs() > EPSILON,
            "the forward pawn moved"
        );
        // The idle pawn's horizontal position is unchanged (gravity may settle it onto
        // the floor vertically, but x/z stay put with no wish input).
        assert!(
            (idler_after.x - idler_before.x).abs() < EPSILON
                && (idler_after.z - idler_before.z).abs() < EPSILON,
            "the idle pawn did not translate horizontally"
        );
    }

    // A jump command keeps its event paired with the pawn that produced it. The
    // caller can therefore select its local pawn before mapping the event into the
    // reaction-address list.
    #[test]
    fn returns_events_with_the_pawn_that_produced_them() {
        let mut registry = EntityRegistry::new();
        let world = floor_world();
        // Give this descriptor a jump so a jump_pressed input produces a "jumped".
        let id = registry.spawn(Transform {
            position: Vec3::new(0.0, 1.21, 0.0),
            ..Transform::default()
        });
        let mut desc = descriptor();
        desc.air.jumps = 1;
        desc.air.jump_velocity = 6.0;
        registry
            .set_component(id, PlayerMovementComponent::from_descriptor(&desc))
            .unwrap();

        // First tick: settle grounded.
        let _ = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(id, idle_input())],
            DT,
        );

        // Second tick: jump.
        let jump = MovementInput {
            jump_pressed: true,
            ..idle_input()
        };
        let events = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(id, jump)],
            DT,
        );
        assert!(
            events
                .iter()
                .any(|(pawn, event)| *pawn == id && event.jumped),
            "a jump command retains its pawn identity and event; got {events:?}"
        );
    }

    // A stale EntityId (no movement component) is skipped without panicking and
    // contributes no events.
    #[test]
    fn skips_pawn_without_movement_component() {
        let mut registry = EntityRegistry::new();
        let world = floor_world();
        // A Transform-only entity (no PlayerMovementComponent).
        let bare = registry.spawn(Transform::default());
        let events = run_host_movement_tick(
            &mut registry,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(bare, forward_input())],
            DT,
        );
        assert!(
            events.is_empty(),
            "a pawn without a movement component contributes nothing"
        );
    }
    fn teleport_world() -> CollisionWorld {
        use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
        use postretro_level_format::teleporters::{
            TeleportMode, TeleporterRecord, TeleportersSection,
        };
        let min = Vec3::splat(-2.0);
        let max = Vec3::splat(2.0);
        let planes = (0..3)
            .flat_map(|axis| {
                let mut normal = Vec3::ZERO;
                normal[axis] = 1.0;
                [
                    EnvironmentPlane {
                        normal: normal.to_array(),
                        distance: max[axis],
                    },
                    EnvironmentPlane {
                        normal: (-normal).to_array(),
                        distance: -min[axis],
                    },
                ]
            })
            .collect();
        let section = TeleportersSection {
            teleporters: vec![TeleporterRecord {
                source_entity: 1,
                mode: TeleportMode::Classic,
                enabled_on_spawn: true,
                brushes: vec![EnvironmentBrush {
                    aabb_min: min.to_array(),
                    aabb_max: max.to_array(),
                    planes,
                }],
                destination_origin: [20.0, 5.0, 10.0],
                destination_yaw: std::f32::consts::FRAC_PI_2,
                landmark_origin: [0.0; 3],
                landmark_yaw: 0.0,
                arrival_sound: String::new(),
                on_teleport: String::new(),
            }],
        };
        let mut world = CollisionWorld::new();
        world.teleporters = postretro_physics::teleporters::Teleporters::from_section(&section);
        world
    }

    #[test]
    fn teleport_host_movement_writes_arrival_yaw_and_mesh_cut_at_every_alpha() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_pawn(&mut registry, Vec3::ZERO);
        registry.snapshot_transforms();
        let world = teleport_world();
        let events = run_host_movement_tick(
            &mut registry,
            &world,
            EnvironmentView::level_only(0.0),
            &[(pawn, idle_input())],
            1.0 / 60.0,
        );
        assert!(events[0].1.teleport.is_some());
        for alpha in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let transform = registry.interpolated_transform(pawn, alpha).unwrap();
            assert!((transform.position - Vec3::new(20.0, 5.0, 10.0)).length() < EPSILON);
            assert!((transform.rotation * Vec3::NEG_Z - Vec3::NEG_X).length() < EPSILON);
        }
    }

    #[test]
    fn teleport_host_fires_after_step_and_respects_bound_armed_state() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_pawn(&mut registry, Vec3::new(-2.05, 0.0, 0.0));
        let mut component = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        component.velocity = Vec3::X * 6.0;
        registry.set_component(pawn, component).unwrap();
        let mut world = teleport_world();
        let teleporter = registry.spawn(Transform::default());
        registry
            .set_component(
                teleporter,
                postretro_entities::TeleporterComponent {
                    index: 0,
                    source_entity: 1,
                    armed: false,
                    arrival_sound: String::new(),
                    on_teleport: String::new(),
                },
            )
            .unwrap();
        world.teleporters.bind_entity(0, teleporter);
        let input = [(pawn, idle_input())];
        let events = run_host_movement_tick(
            &mut registry,
            &world,
            EnvironmentView::level_only(0.0),
            &input,
            1.0 / 60.0,
        );
        assert!(events[0].1.teleport.is_none());
        assert!(
            registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .x
                > -2.0
        );
        let mut armed = registry
            .get_component::<postretro_entities::TeleporterComponent>(teleporter)
            .unwrap()
            .clone();
        armed.armed = true;
        registry.set_component(teleporter, armed).unwrap();
        let events = run_host_movement_tick(
            &mut registry,
            &world,
            EnvironmentView::level_only(0.0),
            &input,
            1.0 / 60.0,
        );
        assert!(events[0].1.teleport.is_some());
    }
}
