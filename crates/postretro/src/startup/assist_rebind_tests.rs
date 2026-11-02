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
