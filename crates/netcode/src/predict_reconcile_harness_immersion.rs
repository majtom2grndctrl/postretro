// The output-only immersion component under predict and reconcile: host movement writes it, and no state intent, replay or wire merge reads it.
// See: context/lib/entity_model.md §7b · context/lib/scripting.md §5

#![cfg(test)]

use glam::Vec3;
use postretro_collision::{BodyProbe, EnvironmentVolume, resolve_environment};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{
    FluidDescriptor, FluidRegistry, ImmersionComponent, MovementStateKind, PlayerMovementComponent,
    SwimParams,
};

use super::predict_reconcile_harness_test_fixtures::{
    LoopbackHarness, NO_CORRECTION_M, NamedEnvironmentVolume, forward_command, gravity_box_volume,
    idle_command, light_link,
};

/// A pool along -Z from the pawn's start, deep enough that a standing pawn
/// swims (immersion ≈ 0.79 against `enter_depth` 0.5); the pawn walks out its
/// far face dry again.
const POOL_MIN: Vec3 = Vec3::new(-50.0, -1.0, -14.0);
const POOL_MAX: Vec3 = Vec3::new(50.0, 1.9, -6.0);
/// The stale values written into the immersion component before every step,
/// alternating, so each differs from the truth on most ticks dry or swimming.
const STALE: [f32; 2] = [1.0, 0.0];

fn swim() -> SwimParams {
    SwimParams {
        speed: 4.5,
        accel: 6.0,
        sink_speed: 0.8,
        enter_depth: 0.5,
        exit_depth: 0.35,
        fluid_jump_velocity: Some(6.5),
    }
}

fn water() -> FluidRegistry {
    FluidRegistry::from_declarations(
        "harness",
        [Ok(FluidDescriptor {
            name: "water".to_string(),
            viscosity: 2.0,
            view_tint: [0.1, 0.3, 0.4, 0.35],
            muffle: 0.8,
            surface_opacity: 0.6,
            enter_sound: None,
            exit_sound: None,
            fog_distance: None,
            fog_color: None,
            wave_period: None,
            wave_amplitude: 8.0,
            wave_length: [64.0, 80.0],
            surface_reflectivity: 1.0,
        })],
    )
}

fn pool() -> Vec<NamedEnvironmentVolume> {
    vec![NamedEnvironmentVolume {
        volume: EnvironmentVolume {
            gravity: None,
            ..gravity_box_volume(POOL_MIN, POOL_MAX, 0.0)
        },
        fluid: Some("water".to_string()),
    }]
}

fn immersion(registry: &EntityRegistry, pawn: EntityId) -> Option<f32> {
    registry
        .get_component::<ImmersionComponent>(pawn)
        .ok()
        .map(|component| component.immersion)
}

// A stale immersion component on both peers changes nothing. The client's
// forward prediction and every reconcile replay (each run with the stale
// value in place) agree with the host to within the no-correction bound, the
// wire merge never overwrites the client's stale value, and host movement
// overwrites the host's with the tick-start resolve its intent used.
#[test]
fn a_stale_immersion_component_changes_no_prediction_replay_or_merge() {
    let mut h = LoopbackHarness::with_fluid_tables(light_link(), pool(), water(), water())
        .with_player_swim(swim());
    h.step_until_armed(&idle_command());
    let client_pawn = h.client_pawn.expect("armed");

    let mut corrections = Vec::new();
    let mut client_swam = false;
    let mut host_swam = false;
    let mut host_writes_checked = 0;
    for step in 0..240_usize {
        let stale = STALE[step % 2];
        h.client_registry
            .set_component(client_pawn, ImmersionComponent { immersion: stale })
            .unwrap();
        h.host_registry
            .set_component(h.host_pawn, ImmersionComponent { immersion: stale })
            .unwrap();
        // The host frame below runs exactly one tick from this pose.
        let host_start = h.host_position();
        let host_capsule = h
            .host_registry
            .get_component::<PlayerMovementComponent>(h.host_pawn)
            .expect("host pawn movement")
            .capsule
            .clone();
        let expected_host_immersion = resolve_environment(
            h.host_environment_view(),
            &BodyProbe {
                position: host_start,
                half_height: host_capsule.half_height,
                radius: host_capsule.radius,
                eye_height: host_capsule.eye_height,
            },
        )
        .immersion;

        h.step(&forward_command(false));
        corrections.extend(h.latest_local_corrections.iter().copied());

        assert_eq!(
            immersion(&h.client_registry, client_pawn),
            Some(stale),
            "step {step}: no merge or replay writes the client's immersion component"
        );
        let host_immersion =
            immersion(&h.host_registry, h.host_pawn).expect("host movement keeps the component");
        assert!(
            (host_immersion - expected_host_immersion).abs() <= f32::EPSILON,
            "step {step}: host movement writes its tick's resolved immersion \
             {expected_host_immersion}, not the stale {stale}; got {host_immersion}"
        );
        host_writes_checked += 1;

        let state = |registry: &EntityRegistry, pawn| {
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .expect("pawn movement")
                .movement_state
                .kind()
        };
        client_swam |= state(&h.client_registry, client_pawn) == MovementStateKind::Swim;
        host_swam |= state(&h.host_registry, h.host_pawn) == MovementStateKind::Swim;
    }

    assert!(host_swam && client_swam, "fixture: both peers must swim");
    assert_eq!(host_writes_checked, 240);
    assert!(
        corrections.len() > 30,
        "reconciles must run throughout; saw {}",
        corrections.len()
    );
    let worst = corrections.iter().copied().fold(0.0_f32, f32::max);
    assert!(
        worst <= NO_CORRECTION_M,
        "every correction must be at most {NO_CORRECTION_M} m; worst {worst}"
    );
}
