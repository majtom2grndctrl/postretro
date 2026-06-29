// Host-side multi-pawn movement seam (M15 Phase 3 Task 4). Advances an explicit
// set of `(EntityId, MovementInput)` pairs through `movement::tick`, writing each
// resulting `Transform` + `PlayerMovementComponent` back to the registry and
// aggregating per-pawn movement events.
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

use glam::Vec3;

use crate::collision::CollisionWorld;
use crate::movement::{MovementInput, tick as movement_tick};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::PlayerMovementComponent;

/// Advance the named movement pawns one fixed tick. Snapshots each pawn's component
/// and position ONCE at the start of the tick, runs `movement::tick` for each, writes
/// the resulting `Transform` plus movement component back, and returns the aggregated
/// movement events (`"landed"` / `"jumped"`) across all pawns in input order. Reading
/// all pawns before writing any keeps each pawn's input applied to its start-of-tick
/// state, never to another pawn's mid-tick write.
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
pub(crate) fn run_host_movement_tick(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    gravity: f32,
    pawn_inputs: &[(EntityId, MovementInput)],
    tick_dt: f32,
) -> Vec<&'static str> {
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
            snapshots.push((*id, component.clone(), transform.position, input.clone()));
        }
    }

    let mut events_out: Vec<&'static str> = Vec::new();
    for (id, mut component, position, input) in snapshots {
        let (new_pos, events) = movement_tick(
            &mut component,
            &input,
            collision_world,
            gravity,
            tick_dt,
            position,
        );
        // Write the advanced pose. `get_component` re-read tolerates a Transform that
        // vanished mid-loop (it cannot here — single-threaded — but keeps the write
        // total).
        if let Ok(transform) = registry.get_component::<Transform>(id) {
            let mut t = *transform;
            t.position = new_pos;
            let _ = registry.set_component(id, t);
        }
        let _ = registry.set_component(id, component);
        if events.landed {
            events_out.push("landed");
        }
        if events.jumped {
            events_out.push("jumped");
        }
    }

    events_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use parry3d::math::{Isometry, Point};
    use parry3d::shape::TriMesh;

    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };

    const EPSILON: f32 = 1e-4;
    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: f32 = -20.0;

    fn floor_world() -> CollisionWorld {
        let points = vec![
            Point::new(-500.0, 0.0, -500.0),
            Point::new(500.0, 0.0, -500.0),
            Point::new(500.0, 0.0, 500.0),
            Point::new(-500.0, 0.0, 500.0),
        ];
        let triangles = vec![[0, 2, 1], [0, 3, 2]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    fn descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
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

        let inputs = vec![(driven, forward_input())];
        let _ = run_host_movement_tick(&mut registry, &world, GRAVITY, &inputs, DT);

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
        assert!(driven_m.is_grounded, "floored pawn grounded after a tick");

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

    // Multiple pawns advance independently in one tick, each by its own input.
    #[test]
    fn advances_each_pawn_by_its_own_input() {
        let mut registry = EntityRegistry::new();
        let world = floor_world();
        let mover = spawn_pawn(&mut registry, Vec3::new(0.0, 1.21, 0.0));
        let idler = spawn_pawn(&mut registry, Vec3::new(5.0, 1.21, 0.0));

        let mover_before = registry.get_component::<Transform>(mover).unwrap().position;
        let idler_before = registry.get_component::<Transform>(idler).unwrap().position;

        let inputs = vec![(mover, forward_input()), (idler, idle_input())];
        let _ = run_host_movement_tick(&mut registry, &world, GRAVITY, &inputs, DT);

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

    // Per-pawn movement events aggregate into one list. A jump command surfaces a
    // "jumped" event from the seam (the same channel simulate_tick feeds
    // TickEvents::movement).
    #[test]
    fn aggregates_per_pawn_movement_events() {
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
        let _ = run_host_movement_tick(&mut registry, &world, GRAVITY, &[(id, idle_input())], DT);

        // Second tick: jump.
        let jump = MovementInput {
            jump_pressed: true,
            ..idle_input()
        };
        let events = run_host_movement_tick(&mut registry, &world, GRAVITY, &[(id, jump)], DT);
        assert!(
            events.contains(&"jumped"),
            "a jump command aggregates a 'jumped' movement event; got {events:?}"
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
            GRAVITY,
            &[(bare, forward_input())],
            DT,
        );
        assert!(
            events.is_empty(),
            "a pawn without a movement component contributes nothing"
        );
    }
}
