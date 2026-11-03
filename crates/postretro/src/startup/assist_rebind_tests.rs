//! The assist pair table binds wherever player events rebuild: on every level
//! install and recompose, replacing the whole table; a connected client binds
//! nothing.
//! See: context/lib/scripting.md §12 (Assist)

use std::collections::BTreeMap;

use postretro_entities::{EntityTypeDescriptor, LifeStatesDescriptor};
use postretro_foundation::{Assistable, LifeStateProfile};

use crate::startup::lifecycle::tests::{script_ctx, test_app};

fn player(assistable: Option<Assistable>) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        canonical_name: Some("player".into()),
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
        life_states: Some(LifeStatesDescriptor {
            initial: "alive".into(),
            states: BTreeMap::from([
                ("alive".into(), LifeStateProfile::default()),
                (
                    "down".into(),
                    LifeStateProfile {
                        assistable,
                        ..Default::default()
                    },
                ),
            ]),
            rejoin: Default::default(),
            state_ids: Default::default(),
            table_revision: 0,
        }),
    }
}

/// `down` is id 1 in the sorted mod-wide name table.
const DOWN: u8 = 1;

fn declares_down(app: &crate::App) -> bool {
    app.session
        .as_ref()
        .expect("test app session installed")
        .assist_state
        .pair_table()
        .declares("player", DOWN)
}

#[test]
fn assist_pair_table_rebinds_with_player_events_and_a_connected_client_binds_nothing() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![player(Some(Assistable::default()))]);
    assert!(!declares_down(&app), "nothing binds before a rebuild");
    app.rebuild_player_events(false);
    assert!(declares_down(&app));

    // A recompose that drops `assistable` replaces the whole table.
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![player(None)]);
    app.rebuild_player_events(true);
    assert!(!declares_down(&app));

    // A connected client never evaluates pair IR, so it binds nothing.
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![player(Some(Assistable::default()))]);
    ctx.owner_slot_writes_enabled.set(false);
    app.rebuild_player_events(false);
    assert!(!declares_down(&app));
}

/// O21 / O51: `unload_level` clears the accumulator's level-scoped state
/// (progress, latches, row-6 records, pair values, stored life states) and
/// keeps the bound table for the next install's rebind.
#[test]
fn assist_unload_level_clears_progress_and_keeps_the_bound_table() {
    use postretro_entities::Transform;
    use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
    use postretro_foundation::{PlayerMovementComponent, Seat};

    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![player(Some(Assistable::default()))]);
    app.rebuild_player_events(false);
    let (helper, target) = {
        let mut registry = ctx.registry.borrow_mut();
        let mut spawn = |seat: u16, down: bool| {
            let pawn = registry.spawn(Transform::default());
            let mut movement = PlayerMovementComponent::from_descriptor(
                &crate::mod_digest::tests::movement_descriptor(),
            );
            if down {
                movement.life_state = DOWN;
                movement.assistable = true;
            }
            registry.set_component(pawn, movement).unwrap();
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
            pawn
        };
        let helper = spawn(0, false);
        let target = spawn(1, true);
        registry.mark_local_player_pawn(helper).unwrap();
        (helper, target)
    };
    let command = crate::build_sim_command(
        &crate::input::ActionSnapshot::neutral(),
        &crate::camera::Camera::new(glam::Vec3::ZERO, 0.0, 0.0),
        crate::sim_intents::SimIntents {
            assist: true,
            ..Default::default()
        },
    );
    {
        let session = app.session.as_mut().expect("test app session installed");
        session
            .assist_state
            .run_host_pass(&mut ctx.registry.borrow_mut(), None, &command, &[]);
        let state = &session.assist_state;
        assert!(state.helper(helper).unwrap().progress.progress > 0);
        assert!(state.pair_values(helper, target).is_some());
    }

    app.unload_level();
    let state = &app.session.as_ref().unwrap().assist_state;
    assert!(state.helper(helper).is_none());
    assert_eq!(state.helpers().count(), 0);
    assert_eq!(state.row6_record(helper), Default::default());
    assert_eq!(state.pair_values(helper, target), None);
    assert!(declares_down(&app), "the bound table survives the unload");
}
