// player.health slot-range follow on descriptor hot reload.

use super::*;

// --- player.health hot-reload range-follow hook ------------------------

use super::super::data_script::follow_pawn_health_range_after_refresh;
use crate::scripting::components::health::HealthComponent;
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::data_descriptors::{
    AirParams, CapsuleParams, FallParams, GroundParams, HealthDescriptor, PlayerMovementDescriptor,
    SpeedParams,
};
use crate::scripting::refresh_plan::{DescriptorRefreshAction, DescriptorRefreshPlan};
use crate::scripting::registry::{ComponentValue, EntityId, EntityRegistry};
use crate::scripting::slot_table::{NumericRange, SlotTable};

fn pawn_movement_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
        capsule: CapsuleParams {
            radius: 0.35,
            half_height: 0.9,
            eye_height: 1.1,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 12.0,
            step_height: 0.35,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.3,
            accel: 2.0,
            max_control_speed: 4.0,
            bunny_hop: true,
            jumps: 1,
            jump_velocity: 5.0,
            jump_ceiling: 2.0,
        },
        fall: FallParams {
            terminal_velocity: 50.0,
        },
        stuck_stop_enabled: true,
        stuck_stop_threshold: 0.001,
        dash: None,
        forgiveness: None,
        crouch: None,
        view_feel: None,
    }
}

/// Spawn a pawn (carries `PlayerMovement`) with a `Health` component whose
/// `max` and `current` are `max`. Returns the pawn id.
fn spawn_pawn(registry: &mut EntityRegistry, max: f32) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            PlayerMovementComponent::from_descriptor(&pawn_movement_descriptor()),
        )
        .unwrap();
    registry
        .set_component(
            id,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max,
                hitbox: None,
                zone_multipliers: std::collections::HashMap::new(),
            }),
        )
        .unwrap();
    id
}

/// A refresh plan that replaces the `Health` component on `entity` with one
/// whose `max` is `new_max`.
fn health_replace_plan(entity: EntityId, new_max: f32) -> DescriptorRefreshPlan {
    DescriptorRefreshPlan {
        actions: vec![DescriptorRefreshAction::Replace {
            entity,
            component: ComponentValue::Health(HealthComponent::from_descriptor(
                &HealthDescriptor {
                    max: new_max,
                    hitbox: None,
                    zone_multipliers: std::collections::HashMap::new(),
                },
            )),
        }],
        diagnostics: Vec::new(),
    }
}

#[test]
fn range_follow_resets_player_health_range_when_pawn_health_replaced() {
    // After a hot reload replaces the pawn's Health (an authored `max`
    // edit), the slot range must follow to `[0, new_max]`. The registry
    // here holds the already-applied (refreshed) component, as it would at
    // the runtime.rs call site.
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, 40.0);
    let plan = health_replace_plan(pawn, 40.0);
    let mut slot_table = SlotTable::new();

    follow_pawn_health_range_after_refresh(&plan, &registry, &mut slot_table);

    assert_eq!(
        slot_table.get("player.health").unwrap().schema.range,
        Some(NumericRange {
            min: 0.0,
            max: 40.0
        }),
    );
}

#[test]
fn range_follow_leaves_range_unchanged_when_pawn_health_untouched() {
    // A plan that replaced some OTHER entity's health (not the resolved
    // pawn) must not move the pawn's slot range. This fixture does not mark
    // a local player pawn, so `pawn_with_health` falls back to the first
    // `PlayerMovement` entity (the lower slot index); the plan here targets
    // the SECOND pawn-shaped entity, so the hook finds no match for the
    // resolved pawn and leaves the range as previously set.
    let mut registry = EntityRegistry::new();
    let _first_pawn = spawn_pawn(&mut registry, 100.0);
    let second = spawn_pawn(&mut registry, 30.0);
    let mut slot_table = SlotTable::new();
    slot_table
        .set_engine_numeric_range(
            "player.health",
            NumericRange {
                min: 0.0,
                max: 77.0,
            },
        )
        .unwrap();

    let plan = health_replace_plan(second, 5.0);
    follow_pawn_health_range_after_refresh(&plan, &registry, &mut slot_table);

    assert_eq!(
        slot_table.get("player.health").unwrap().schema.range,
        Some(NumericRange {
            min: 0.0,
            max: 77.0
        }),
        "range untouched when the plan did not replace the resolved pawn's health",
    );
}

#[test]
fn range_follow_noop_without_pawn() {
    // No pawn at all: the hook must not panic and must leave the range as-is.
    let registry = EntityRegistry::new();
    let plan = DescriptorRefreshPlan {
        actions: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut slot_table = SlotTable::new();

    follow_pawn_health_range_after_refresh(&plan, &registry, &mut slot_table);

    assert_eq!(slot_table.get("player.health").unwrap().schema.range, None);
}
