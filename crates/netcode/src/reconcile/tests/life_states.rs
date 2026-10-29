//! Identity forcing is independent of table availability and correction magnitude.
use super::*;

fn movement_mut(
    registry: &mut postretro_entities::EntityRegistry,
    pawn: postretro_entities::EntityId,
) -> &mut PlayerMovementComponent {
    let postretro_entities::ComponentValue::PlayerMovement(movement) = registry
        .get_component_value_mut(pawn, postretro_entities::ComponentKind::PlayerMovement)
        .unwrap()
    else {
        panic!("fixture pawn has movement");
    };
    movement.as_mut()
}

#[test]
fn life_state_unknown_revision_large_correction_and_composition_glide() {
    let world = floor_world();
    let mut registry = EntityRegistry::new();
    let mut prediction = ClientPrediction::new();
    let pawn = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(1));
    assert!(!prediction.seed_presentation_offset(Vec3::X));
    let mut wire = authoritative_movement();
    wire.life_state = 0;
    wire.life_state_table_revision = 9; // deliberately not installed: gate keeps old identity
    let delta = TELEPORT_CORRECTION_MIN_M * 2.0;
    let class = reconcile_local_pawn(
        &mut registry,
        &mut prediction,
        pawn,
        Transform {
            position: START - Vec3::X * delta,
            ..Default::default()
        },
        Some(&wire),
        Some(0),
        &world,
        postretro_collision::EnvironmentView::level_only(GRAVITY),
        DT,
    )
    .unwrap();
    assert_eq!(class, CorrectionClass::OversizedSmoothed);
    assert_eq!(prediction.presentation_offset(), Vec3::X * (delta + 1.0));
    assert!(prediction.teleport_adoptions.is_empty());
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state,
        postretro_foundation::UNDECLARED_LIFE_STATE
    );
    // Another ordinary offset cannot turn this still-large glide into an eye cut.
    assert!(!prediction.seed_presentation_offset(Vec3::X * 0.01));
}

#[test]
fn life_state_equal_identity_keeps_magnitude_snap_and_real_teleport_still_cuts() {
    for changed_identity in [false, true] {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let pawn = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(1));
        let mut wire = authoritative_movement();
        if changed_identity {
            wire.life_state = 0;
            wire.teleport.generation = 1;
        }
        let class = reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: START + Vec3::X * TELEPORT_CORRECTION_MIN_M * 2.0,
                ..Default::default()
            },
            Some(&wire),
            Some(0),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        )
        .unwrap();
        assert_eq!(class, CorrectionClass::Teleport);
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
    }
}

#[test]
fn life_state_revision_only_change_forces_large_held_record_to_glide() {
    let world = floor_world();
    let mut registry = EntityRegistry::new();
    let mut prediction = ClientPrediction::new();
    let pawn = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(1));
    postretro_foundation::enter_life_state(
        movement_mut(&mut registry, pawn),
        0,
        &postretro_foundation::LifeStateProfile::default(),
        8,
    );
    let mut wire = authoritative_movement();
    wire.life_state = 0;
    wire.life_state_table_revision = 9;
    let class = reconcile_local_pawn(
        &mut registry,
        &mut prediction,
        pawn,
        Transform {
            position: START - Vec3::X * TELEPORT_CORRECTION_MIN_M * 2.0,
            ..Default::default()
        },
        Some(&wire),
        Some(0),
        &world,
        postretro_collision::EnvironmentView::level_only(GRAVITY),
        DT,
    )
    .unwrap();
    assert_eq!(class, CorrectionClass::OversizedSmoothed);
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state_table_revision,
        8
    );
    assert!(prediction.presentation_offset().length() > TELEPORT_CORRECTION_MIN_M);
}
