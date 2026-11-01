//! Life-state table history through the production install: the endpoint pins
//! the armed pawn's revision, then the tuning rebuild restamps under it.
//! See: context/lib/networking.md §What gates, and what replicates instead
use super::*;
use crate::collision::CollisionWorld;
use crate::predict_reconcile_harness_test_fixtures::{GRAVITY, floor_world, player_descriptor};
use postretro_foundation::{LifeStateProfile, PlayerMovementComponent};

fn life_payload(revision: u8, profiles: Vec<(u8, LifeStateProfile)>) -> TuningPayload {
    let mut payload = TuningPayload::new(
        Some(player_descriptor()),
        std::array::from_fn(|_| None),
        environment(),
    );
    payload.life_state_names = vec!["alive".into(), "down".into(), "x".into()];
    payload.life_state_profiles = profiles;
    payload.life_state_table_revision = revision;
    payload
}

/// Install `payload`, then arm and adopt the local pawn as the first owner
/// record does.
fn armed_client(payload: &TuningPayload) -> (NetEndpoint, EntityRegistry, EntityId) {
    let mut endpoint = client_endpoint();
    let mut registry = EntityRegistry::new();
    install(&mut endpoint, &mut registry, &bytes(payload));
    let pawn = arm(&mut endpoint, &mut registry);
    let NetEndpoint::Client { prediction, .. } = &mut endpoint else {
        unreachable!()
    };
    prediction.arm(PAWN, pawn);
    prediction.mark_first_record_adopted();
    (endpoint, registry, pawn)
}

/// Merge an owner record naming `(state, revision, elapsed)` through the
/// production reconcile, which runs the table-history gate.
fn merge(
    endpoint: &mut NetEndpoint,
    registry: &mut EntityRegistry,
    pawn: EntityId,
    world: &CollisionWorld,
    server_tick: u32,
    (state, revision, elapsed): (u8, u8, u32),
) {
    let mut wire = movement_state::movement_state_to_wire(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap(),
        0.0,
    );
    wire.life_state = state;
    wire.life_state_table_revision = revision;
    wire.life_state_elapsed_ticks = elapsed;
    let transform = *registry.get_component::<Transform>(pawn).unwrap();
    let NetEndpoint::Client {
        prediction,
        life_state_tables,
        ..
    } = endpoint
    else {
        unreachable!()
    };
    reconcile::reconcile_local_pawn_with_mover_history(
        registry,
        prediction,
        pawn,
        transform,
        Some(&wire),
        Some(0),
        server_tick,
        None,
        world,
        postretro_collision::EnvironmentView::level_only(GRAVITY),
        postretro_foundation::TICK_DURATION.as_secs_f32(),
        life_state_tables,
    )
    .unwrap();
}

fn pawn_movement(registry: &EntityRegistry, pawn: EntityId) -> PlayerMovementComponent {
    registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone()
}

fn history_has(endpoint: &NetEndpoint, revision: u8) -> bool {
    let NetEndpoint::Client {
        life_state_tables, ..
    } = endpoint
    else {
        unreachable!()
    };
    life_state_tables.table(revision).is_some()
}

#[test]
fn life_state_o36_endpoint_installs_past_the_pawns_revision_restamp_under_its_pinned_table() {
    const R: u8 = 10;
    let mut down = LifeStateProfile::default();
    down.fire = false;
    down.movement.speed_scale = 0.3;
    let mut x = LifeStateProfile::default();
    x.fire = false;
    x.movement.speed_scale = 0.5;
    let (mut endpoint, mut registry, pawn) = armed_client(&life_payload(R, vec![(1, down)]));
    let world = floor_world();
    merge(&mut endpoint, &mut registry, pawn, &world, 1, (1, R, 50));
    assert!(!pawn_movement(&registry, pawn).life_fire);

    // r+1 and r+2 both install before a record at either merges. r+3 and r+4
    // push r out of the four-entry recent window, so only the endpoint's pin
    // read keeps table r alive for the rebuild each install triggers.
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&life_payload(R + 1, vec![(0, LifeStateProfile::default())])),
    );
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&life_payload(R + 2, vec![(2, x)])),
    );
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&life_payload(R + 3, vec![])),
    );
    // A stale stamp the last rebuild must replace from table r, not carry.
    let mut stale = pawn_movement(&registry, pawn);
    stale.life_fire = true;
    stale.life_speed_scale = 1.0;
    registry.set_component(pawn, stale).unwrap();
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&life_payload(R + 4, vec![])),
    );
    let NetEndpoint::Client {
        tuning_generation,
        applied_movement_tuning_generation,
        ..
    } = &endpoint
    else {
        unreachable!()
    };
    assert_eq!(
        tuning_generation, applied_movement_tuning_generation,
        "each install drives the production tuning rebuild"
    );
    assert!(history_has(&endpoint, R), "the pin keeps the pawn's table");
    let rebuilt = pawn_movement(&registry, pawn);
    assert_eq!(
        (
            rebuilt.life_state,
            rebuilt.life_state_table_revision,
            rebuilt.life_state_elapsed_ticks
        ),
        (1, R, 50)
    );
    assert!(!rebuilt.life_fire);
    assert!((rebuilt.life_speed_scale - 0.3).abs() < f32::EPSILON);

    // Each record restamps under its own revision's table.
    merge(&mut endpoint, &mut registry, pawn, &world, 2, (0, R + 1, 5));
    let first = pawn_movement(&registry, pawn);
    assert_eq!(
        (first.life_state, first.life_state_table_revision),
        (0, R + 1)
    );
    assert!(first.life_fire);
    assert!((first.life_speed_scale - 1.0).abs() < f32::EPSILON);
    merge(&mut endpoint, &mut registry, pawn, &world, 3, (2, R + 2, 6));
    let second = pawn_movement(&registry, pawn);
    assert_eq!(
        (
            second.life_state,
            second.life_state_table_revision,
            second.life_state_elapsed_ticks
        ),
        (2, R + 2, 6)
    );
    assert!(!second.life_fire);
    assert!((second.life_speed_scale - 0.5).abs() < f32::EPSILON);

    // Once the pawn has moved on, the next install reads the new pin and the
    // old table falls out of the window.
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&life_payload(R + 5, vec![])),
    );
    assert!(!history_has(&endpoint, R));
    assert!(history_has(&endpoint, R + 2));
}

#[test]
fn life_state_o42_endpoint_held_revision_drop_keeps_live_stamps_until_the_host_remap() {
    const R: u8 = 4;
    let mut x = LifeStateProfile::default();
    x.fire = false;
    x.movement.speed_scale = 0.4;
    let (mut endpoint, mut registry, pawn) = armed_client(&life_payload(
        R,
        vec![(0, LifeStateProfile::default()), (2, x)],
    ));
    let world = floor_world();
    merge(&mut endpoint, &mut registry, pawn, &world, 1, (2, R, 60));
    assert!(!pawn_movement(&registry, pawn).life_fire);

    // Another descriptor keeps `x` in the names, so the revision holds while
    // this pawn's profile for it is gone.
    let generation_before = match &endpoint {
        NetEndpoint::Client {
            tuning_generation, ..
        } => *tuning_generation,
        NetEndpoint::Host { .. } => unreachable!(),
    };
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&life_payload(R, vec![(0, LifeStateProfile::default())])),
    );
    let NetEndpoint::Client {
        tuning_generation,
        applied_movement_tuning_generation,
        life_state_tables,
        ..
    } = &endpoint
    else {
        unreachable!()
    };
    assert_ne!(*tuning_generation, generation_before);
    assert_eq!(tuning_generation, applied_movement_tuning_generation);
    let entry = life_state_tables.table(R).expect("held revision installed");
    assert!(entry.profile(2).is_none(), "the payload replaced the entry");
    let rebuilt = pawn_movement(&registry, pawn);
    assert_eq!(
        (
            rebuilt.life_state,
            rebuilt.life_state_table_revision,
            rebuilt.life_state_elapsed_ticks
        ),
        (2, R, 60)
    );
    assert!(!rebuilt.life_fire, "no profile for x: live stamps stand");
    assert!((rebuilt.life_speed_scale - 0.4).abs() < f32::EPSILON);

    // A record at the held revision still naming x keeps the pre-merge state.
    merge(&mut endpoint, &mut registry, pawn, &world, 2, (2, R, 62));
    let stale = pawn_movement(&registry, pawn);
    assert_eq!(
        (
            stale.life_state,
            stale.life_state_table_revision,
            stale.life_state_elapsed_ticks
        ),
        (2, R, 60)
    );
    assert!(!stale.life_fire);

    // The host's remapped `initial` restamps from the replaced entry.
    merge(&mut endpoint, &mut registry, pawn, &world, 3, (0, R, 0));
    let remapped = pawn_movement(&registry, pawn);
    assert_eq!(
        (remapped.life_state, remapped.life_state_table_revision),
        (0, R)
    );
    assert!(remapped.life_fire);
    assert!((remapped.life_speed_scale - 1.0).abs() < f32::EPSILON);
}
