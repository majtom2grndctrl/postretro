use postretro_combat_model::{EnvironmentTuning, TuningPayload, WieldableTuningPayload};
use postretro_entities::EntityTypeDescriptor;
use postretro_foundation::{
    ActivationTrigger, BoolOrIr, CrouchParams, DashParams, NumberOrIr, PlayerMovementDescriptor,
    ResolutionMode, WeaponActivationDescriptor, WeaponDescriptor, WeaponResource,
};

use super::{registry_facts, tuning_facts};
use crate::input::{
    BindingSources, BindingState, Command, DeviceClass, InputSystem, default_bindings,
};

fn dash() -> DashParams {
    DashParams {
        boost_speed: NumberOrIr::Literal(12.0),
        momentum_retention: NumberOrIr::Literal(0.5),
        steer_control: NumberOrIr::Literal(0.3),
        dash_drag: NumberOrIr::Literal(2.0),
        cooldown_ms: NumberOrIr::Literal(400.0),
        air_dashes: 1,
        preserve_vertical: BoolOrIr::Literal(false),
    }
}

fn movement(with_dash: bool, with_crouch: bool) -> PlayerMovementDescriptor {
    let mut movement = crate::mod_digest::tests::movement_descriptor();
    movement.dash = with_dash.then(dash);
    movement.crouch = with_crouch.then_some(CrouchParams {
        half_height: 0.5,
        eye_height: 0.3,
        transition_rate: 8.0,
    });
    movement
}

fn entity(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

fn weapon(resource: Option<WeaponResource>, secondary: bool) -> WeaponDescriptor {
    let mut value = serde_json::json!({
        "damage": 10, "range": 100, "resolution": "hitscan",
        "primary": { "trigger": "press", "recoveryMs": 300,
            "steps": [{ "kind": "shot" }] },
    });
    if secondary {
        value["secondary"] = value["primary"].clone();
    }
    let mut descriptor: WeaponDescriptor =
        serde_json::from_value(value).expect("minimal weapon descriptor parses");
    descriptor.resource = resource;
    descriptor
}

fn magazine() -> WeaponResource {
    serde_json::from_value(serde_json::json!({
        "kind": "ammo", "type": "rounds", "magazine": 6, "reserve": 0
    }))
    .expect("ammo resource parses")
}

fn heat() -> WeaponResource {
    serde_json::from_value(serde_json::json!({
        "kind": "heat", "heatPerShot": 0.1, "overheatAt": 1.0, "coolPerSecond": 0.5
    }))
    .expect("heat resource parses")
}

#[test]
fn dash_is_relevant_when_any_of_two_movement_descriptors_has_it() {
    let mut without = entity("walker");
    without.movement = Some(movement(false, false));
    assert!(!registry_facts(&[without.clone()]).dash);
    let mut with = entity("dasher");
    with.movement = Some(movement(true, false));
    assert!(registry_facts(&[without, with]).dash);
}

#[test]
fn reload_crouch_and_alt_fire_follow_their_descriptors() {
    let mut crouching = entity("player");
    crouching.movement = Some(movement(false, true));
    let facts = registry_facts(&[crouching]);
    assert!(facts.crouch && !facts.magazine && !facts.secondary);

    let mut rifle = entity("rifle");
    rifle.weapon = Some(weapon(Some(magazine()), false));
    let facts = registry_facts(&[rifle]);
    assert!(facts.magazine && !facts.secondary && !facts.crouch);

    let mut beam = entity("beam");
    beam.weapon = Some(weapon(Some(heat()), true));
    let facts = registry_facts(&[beam]);
    assert!(!facts.magazine && facts.secondary);
}

#[test]
fn a_one_weapon_magazine_or_secondary_survives_a_second_weapon_without() {
    let mut rifle = entity("rifle");
    rifle.weapon = Some(weapon(Some(magazine()), true));
    let mut knife = entity("knife");
    knife.weapon = Some(weapon(None, false));
    let facts = registry_facts(&[rifle, knife]);
    assert!(facts.magazine && facts.secondary);
}

fn tuning_with_dash() -> TuningPayload {
    TuningPayload::new(
        Some(movement(true, false)),
        std::array::from_fn(|_| None),
        EnvironmentTuning {
            level_gravity: -9.81,
            fluids: Default::default(),
        },
    )
}

#[test]
fn host_tuning_makes_dash_relevant_for_a_client_registry_without_it() {
    let local = registry_facts(&[]);
    assert!(!local.dash);
    assert!(local.union(tuning_facts(&tuning_with_dash())).dash);
}

#[test]
fn host_tuning_wieldables_carry_magazine_and_secondary() {
    let mut slots: [Option<WieldableTuningPayload>; _] = std::array::from_fn(|_| None);
    let rifle = WieldableTuningPayload {
        canonical_name: "rifle".to_string(),
        placement: postretro_foundation::LEGACY_WEAPON_PLACEMENT,
        muzzle_offset: None,
        range: 64.0,
        primary: WeaponActivationDescriptor::single(ActivationTrigger::Press, 100.0),
        secondary: Some(WeaponActivationDescriptor::single(
            ActivationTrigger::Press,
            100.0,
        )),
        damage: 10.0,
        knockback: None,
        projectile: None,
        splash: None,
        resource: Some(magazine()),
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        resolution: ResolutionMode::Hitscan,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    };
    slots[0] = Some(rifle);
    let facts = tuning_facts(&TuningPayload::new(
        None,
        slots,
        EnvironmentTuning {
            level_gravity: -9.81,
            fluids: Default::default(),
        },
    ));
    assert!(facts.magazine && facts.secondary && !facts.dash);
}

#[test]
fn dash_binds_while_host_tuning_is_installed_and_unbinds_after_demote() {
    // P5: the table rebuilds when tuning clears, not only when it installs.
    let mut state = BindingState::default();
    let mut input = InputSystem::new(default_bindings());
    let local = registry_facts(&[]);
    let solo = BindingSources {
        entity_types_generation: 1,
        tuning: None,
    };
    state.rebuild(solo, local, &mut input);
    let dash_bound = |state: &BindingState| {
        !state
            .table()
            .inputs(Command::Dash, DeviceClass::KeyboardMouse)
            .is_empty()
    };
    assert!(!dash_bound(&state));

    let participating = BindingSources {
        entity_types_generation: 1,
        tuning: Some(7),
    };
    assert!(state.needs_rebuild(participating));
    state.rebuild(
        participating,
        local.union(tuning_facts(&tuning_with_dash())),
        &mut input,
    );
    assert!(dash_bound(&state));

    // A demote clears tuning without bumping its generation.
    assert!(state.needs_rebuild(solo));
    state.rebuild(solo, local, &mut input);
    assert!(!dash_bound(&state));
}

#[test]
fn the_controls_panel_lists_dash_from_the_registry_before_any_level_loads() {
    // P6, R5: opening the panel from the frontend reads a table built from the
    // committed registry, with no level loaded.
    for with_dash in [true, false] {
        let mut app = crate::app::controls_panel::tests::test_app();
        let mut pawn = entity("pawn");
        pawn.movement = Some(movement(with_dash, false));
        app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_ctx
            .data_registry
            .borrow_mut()
            .replace_entity_types(vec![pawn]);
        app.open_controls_panel();
        let session = app.session.as_ref().unwrap();
        let rows = crate::app::controls_panel::controls_rows(
            session.bindings.table(),
            session.bindings.author(),
        );
        assert_eq!(
            rows.iter().any(|row| row.command == Command::Dash),
            with_dash
        );
    }
}

// --- `assist` relevance (E18 revive-assist AC7, AC8) ---

fn life_states(assistable: bool) -> postretro_entities::data_descriptors::LifeStatesDescriptor {
    use postretro_foundation::{Assistable, LifeStateProfile};
    postretro_entities::data_descriptors::LifeStatesDescriptor {
        initial: "alive".into(),
        states: [
            ("alive".to_string(), LifeStateProfile::default()),
            (
                "down".to_string(),
                LifeStateProfile {
                    assistable: assistable.then(Assistable::default),
                    ..LifeStateProfile::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        rejoin: Default::default(),
        state_ids: Default::default(),
        table_revision: 0,
    }
}

#[test]
fn assist_is_relevant_exactly_when_a_loaded_life_state_declares_assistable() {
    let mut plain = entity("plain");
    plain.life_states = Some(life_states(false));
    assert!(!registry_facts(&[plain.clone(), entity("none")]).assistable);
    let mut reviver = entity("reviver");
    reviver.life_states = Some(life_states(true));
    assert!(registry_facts(&[plain, reviver]).assistable);
}

#[test]
fn host_tuning_life_state_profiles_make_assist_relevant_in_co_op() {
    let mut tuning = tuning_with_dash();
    assert!(!tuning_facts(&tuning).assistable);
    tuning.life_state_profiles = vec![(
        1,
        postretro_foundation::LifeStateProfile {
            assistable: Some(Default::default()),
            ..Default::default()
        },
    )];
    let facts = tuning_facts(&tuning);
    assert!(facts.assistable && facts.dash, "the other facts survive");
    assert!(registry_facts(&[]).union(facts).assistable);
}

/// Install `pawn` as the mod's only entity type in a fresh session and run the
/// Input stage's refresh, as the first gameplay frame does before it reads a
/// snapshot.
fn session_after_first_refresh(pawn: EntityTypeDescriptor) -> crate::App {
    let mut app = crate::app::controls_panel::tests::test_app();
    app.session
        .as_ref()
        .unwrap()
        .scripting
        .script_ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![pawn]);
    app.refresh_effective_bindings();
    app
}

#[test]
fn irrelevant_assist_session_first_gameplay_frame_fires_use_on_press() {
    use crate::input::{Action, ButtonState};
    use winit::keyboard::KeyCode;
    // The boot seed is relevance-blind: on it, Use on E would wait for a
    // release beside the engine `hold`. The refresh replaces it first.
    let mut boot = InputSystem::new(default_bindings());
    boot.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    assert_eq!(
        boot.snapshot_at(0.01).button(Action::Use),
        ButtonState::Inactive
    );

    let mut pawn = entity("pawn");
    pawn.life_states = Some(life_states(false));
    let mut app = session_after_first_refresh(pawn);
    let session = app.session.as_mut().unwrap();
    let rows = crate::app::controls_panel::controls_rows(
        session.bindings.table(),
        session.bindings.author(),
    );
    assert!(rows.iter().all(|row| row.command != Command::Assist));
    let input = &mut session.input_system;
    let t = input.now();
    input.handle_keyboard_event_at(KeyCode::KeyE, true, t);
    let snapshot = input.snapshot_at(t + 0.01);
    assert_eq!(snapshot.button(Action::Use), ButtonState::Pressed);
    assert!(!snapshot.button(Action::Assist).is_active());
}

#[test]
fn relevant_assist_session_lists_assist_and_holds_it_on_key_e() {
    use crate::input::{Action, ButtonState};
    use winit::keyboard::KeyCode;
    let mut pawn = entity("pawn");
    pawn.life_states = Some(life_states(true));
    let mut app = session_after_first_refresh(pawn);
    let session = app.session.as_mut().unwrap();
    let rows = crate::app::controls_panel::controls_rows(
        session.bindings.table(),
        session.bindings.author(),
    );
    assert!(rows.iter().any(|row| row.command == Command::Assist));
    let input = &mut session.input_system;
    let t = input.now();
    input.handle_keyboard_event_at(KeyCode::KeyE, true, t);
    let pending = input.snapshot_at(t + 0.01);
    assert_eq!(pending.button(Action::Use), ButtonState::Inactive);
    let held = input.snapshot_at(t + 0.3);
    assert!(held.button(Action::Assist).is_active());
    assert_eq!(held.button(Action::Use), ButtonState::Inactive);
}
