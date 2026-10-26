// Host-sampled weapon timers cross the real app/UI boundary without changing
// the store, and every UI consumer resolves the same display target. CPU only.
// See: context/lib/ui.md §3 · context/lib/networking.md §Combat authority

use postretro_entities::SlotValue;
use postretro_net::state_slots::WireSlotValue;
use postretro_ui::UiReadSnapshot;
use postretro_ui::descriptor::AnchoredTree;
use postretro_ui::modal_stack::{ModalStack, ScopeTier};
use postretro_ui::theme::UiTheme;
use postretro_ui::tree::{ImageSizes, ScrollInput, TweenClock, UiDrawData, UiTree};
use serde_json::{Value, json};

use crate::App;
use crate::netcode::{NetEndpoint, NetRole, ReplicatedSlotIdentity};
use crate::startup::lifecycle::tests::test_app;

const DEVICE: [u32; 2] = [1280, 720];
const TIMED: [&str; 4] = [
    "player.reloadProgress",
    "player.weaponCooldownMs",
    "player.heat",
    "player.cell",
];

fn endpoint(role: NetRole) -> Option<NetEndpoint> {
    NetEndpoint::from_role(&role, None).expect("ephemeral test endpoint constructs")
}

fn sample(app: &mut App, sequence: u32, tick: u32, values: &[(&str, WireSlotValue)]) {
    let session = app.session.as_mut().unwrap();
    let Some(NetEndpoint::Client { state_slots, .. }) = session.net_endpoint.as_mut() else {
        panic!("sample fixture is a client");
    };
    state_slots.apply_timed_slot_snapshot_for_test(
        &mut session.scripting.script_ctx.slot_table.borrow_mut(),
        &ReplicatedSlotIdentity::default(),
        sequence,
        tick,
        values,
    );
}

fn advance(app: &mut App, tick: f64, dt: f64) {
    let Some(NetEndpoint::Client { state_slots, .. }) = app
        .session
        .as_mut()
        .and_then(|session| session.net_endpoint.as_mut())
    else {
        panic!("advance fixture is a client");
    };
    state_slots.advance_timed_slot_targets(Some(tick), dt);
}

fn correlated(value: f32) -> WireSlotValue {
    WireSlotValue::Array(vec![0.0, value])
}

/// Three accepted host samples establish both intervals of each ramp, then a
/// rendered frame with no snapshot advances its display target.
fn ramping_client() -> App {
    let mut app = test_app();
    app.session.as_mut().unwrap().net_endpoint = endpoint(NetRole::Connect {
        addr: "127.0.0.1:1".parse().unwrap(),
    });
    advance(&mut app, 10.0, 0.0);
    for (sequence, reload, cooldown, heat, cell) in [
        (0, 0.125, 128.0, 96.0, 16.0),
        (1, 0.25, 96.0, 80.0, 24.0),
        (2, 0.375, 64.0, 64.0, 32.0),
    ] {
        let tick = 10 + 2 * sequence;
        sample(
            &mut app,
            sequence,
            tick,
            &[
                ("player.reloadProgress", correlated(reload)),
                ("player.reloadActive", correlated(1.0)),
                ("player.weaponCooldownMs", correlated(cooldown)),
                ("player.heat", correlated(heat)),
                ("player.overheatAt", correlated(128.0)),
                ("player.cell", correlated(cell)),
                ("player.cellCapacity", correlated(128.0)),
            ],
        );
        advance(
            &mut app,
            f64::from(tick),
            if sequence == 0 { 0.0 } else { 2.0 / 60.0 },
        );
    }
    advance(&mut app, 15.0, 1.0 / 60.0);
    app
}

fn tree(root: Value) -> AnchoredTree {
    serde_json::from_value(json!({
        "anchor": "topLeft", "offset": [0.0, 0.0], "root": root,
    }))
    .expect("UI fixture descriptor parses")
}

fn snapshot(app: &mut App, descriptor: &AnchoredTree, now: f64) -> UiReadSnapshot {
    let session = app.session.as_mut().unwrap();
    let mut stack = ModalStack::new();
    stack
        .registry_mut()
        .register("timed", descriptor.clone(), ScopeTier::Mod, true);
    App::build_ui_read_snapshot(
        &stack,
        &mut session.presentation_cells,
        &session.scripting.script_ctx.slot_table.borrow(),
        session.net_endpoint.as_ref(),
        now,
        session.ui_input_mode,
        None,
        false,
    )
}

fn number(snapshot: &UiReadSnapshot, slot: &str) -> f32 {
    match snapshot.slot_values.get(slot) {
        Some(SlotValue::Number(value)) => *value,
        value => panic!("{slot} should be numeric, got {value:?}"),
    }
}

fn draw(ui: &mut UiTree, snapshot: &UiReadSnapshot) -> UiDrawData {
    ui.build_draw_data_retained_with_image_generation(
        DEVICE,
        &mut postretro_ui::text::build_font_system(),
        &ImageSizes::new(),
        0,
        &snapshot.slot_values,
        &snapshot.cell_values,
        TweenClock {
            now: snapshot.time_seconds,
            snap: snapshot.reduce_motion,
        },
        ScrollInput::default(),
    )
}

fn bar(bind: Value, max: Value) -> Value {
    json!({
        "kind": "bar", "bind": bind, "max": max,
        "width": 128.0, "height": 12.0,
        "fill": [0.0, 1.0, 0.0, 1.0], "background": [0.1, 0.1, 0.1, 1.0],
    })
}

#[test]
fn timed_slot_ui_snapshot_drives_binding_predicate_max_focus_and_accessibility() {
    let mut app = ramping_client();
    let empty = tree(json!({"kind":"spacer", "flexGrow":0.0}));
    let targets = snapshot(&mut app, &empty, 0.0);
    let heat = number(&targets, "player.heat");
    assert!(heat < 64.0);
    assert!(number(&targets, "player.reloadProgress") > 0.375);
    assert!(number(&targets, "player.weaponCooldownMs") < 64.0);
    assert!(number(&targets, "player.cell") > 32.0);

    let descriptor = tree(json!({
        "kind":"vstack", "gap":4.0, "padding":0.0, "align":"start",
        "children":[
            {"kind":"text", "content":"raw", "fontSize":18.0,
             "color":[1.0,1.0,1.0,1.0],
             "bind":{"slot":"player.reloadProgress", "decimalPlaces":4}},
            bar(json!({"slot":"player.cell"}), json!({"slot":"player.weaponCooldownMs"})),
            {"kind":"button", "id":"projected", "label":"Projected", "onPress":"noop",
             "visibleWhen":{"slot":"player.heat", "equals":heat},
             "checked":{"slot":"player.heat", "equals":heat}}
        ],
    }));
    let projected = snapshot(&mut app, &descriptor, 0.0);
    let mut ui = UiTree::from_descriptor(&descriptor, &UiTheme::engine_default());
    let drawn = draw(&mut ui, &projected);
    assert!(
        drawn
            .texts
            .iter()
            .any(|text| text.content
                == format!("{:.4}", number(&projected, "player.reloadProgress")))
    );
    let expected_width =
        128.0 * number(&projected, "player.cell") / number(&projected, "player.weaponCooldownMs");
    assert!(
        (drawn.quads.instances[1].rect[2] - expected_width.round()).abs() < 1e-3,
        "Bar numerator and state-bound maximum must both read the projection"
    );
    let focus = ui.export_focus_rects(
        &descriptor,
        DEVICE,
        &projected.slot_values,
        &projected.cell_values,
    );
    let button = focus
        .rects
        .iter()
        .find(|rect| rect.id == "projected")
        .expect("projected predicate makes the button reachable for focus and hit-testing");
    assert_eq!(
        button.checked,
        Some(1.0),
        "accessibility state reads the same projection"
    );
    assert!(button.rect[2] > 0.0 && button.rect[3] > 0.0);

    // Every other slot remains sampled; the four projected values never feed
    // back into the store used by gameplay and crossing detection.
    let session = app.session.as_ref().unwrap();
    let raw = App::build_ui_slot_snapshot(&session.scripting.script_ctx.slot_table.borrow());
    for (name, value) in &raw {
        if !TIMED.contains(&name.as_str()) {
            assert_eq!(projected.slot_values.get(name), Some(value), "{name}");
        }
    }
    assert_eq!(raw["player.reloadProgress"], SlotValue::Number(0.375));
    assert_eq!(raw["player.weaponCooldownMs"], SlotValue::Number(64.0));
    assert_eq!(raw["player.heat"], SlotValue::Number(64.0));
    assert_eq!(raw["player.cell"], SlotValue::Number(32.0));

    // The same live store produces raw targets on the other two roles.
    for role in [NetRole::SinglePlayer, NetRole::Host { port: 0 }] {
        app.session.as_mut().unwrap().net_endpoint = endpoint(role);
        assert_eq!(snapshot(&mut app, &descriptor, 0.0).slot_values, raw);
    }
}

#[test]
fn timed_slot_ui_tween_follows_moving_target_and_reduce_motion_snaps_this_frame() {
    let mut app = ramping_client();
    let descriptor = tree(bar(
        json!({
            "slot":"player.reloadProgress",
            "tween":{"durationMs":1000.0,"easing":"linear","from":0.0},
        }),
        json!(1.0),
    ));
    let mut ui = UiTree::from_descriptor(&descriptor, &UiTheme::engine_default());
    draw(&mut ui, &snapshot(&mut app, &descriptor, 0.0));
    advance(&mut app, 16.0, 1.0 / 60.0);
    let moving = snapshot(&mut app, &descriptor, 0.5);
    let eased = draw(&mut ui, &moving);
    let width = eased.quads.instances[1].rect[2];
    assert!(width > 0.0 && width < 128.0 * number(&moving, "player.reloadProgress"));
    app.session
        .as_mut()
        .unwrap()
        .scripting
        .script_ctx
        .slot_table
        .borrow_mut()
        .get_mut("accessibility.reduceMotion")
        .unwrap()
        .value = Some(SlotValue::Boolean(true));
    advance(&mut app, 17.0, 1.0 / 60.0);
    let reduced = snapshot(&mut app, &descriptor, 0.6);
    assert!(reduced.reduce_motion);
    let snapped = draw(&mut ui, &reduced);
    assert!(
        (snapped.quads.instances[1].rect[2]
            - (128.0 * number(&reduced, "player.reloadProgress")).round())
        .abs()
            < 1e-3
    );
    assert!(number(&reduced, "player.reloadProgress") > number(&moving, "player.reloadProgress"));
}

#[test]
fn timed_slot_ui_exit_fade_captures_host_fall_after_completion_in_same_frame() {
    let mut app = ramping_client();
    let mut root = bar(json!({"slot":"player.reloadProgress"}), json!(1.0));
    root["visibleWhen"] = json!({"slot":"player.reloadActive","equals":true});
    root["exitFade"] = json!({"durationMs":500.0});
    let descriptor = tree(root);
    let mut ui = UiTree::from_descriptor(&descriptor, &UiTheme::engine_default());
    assert_eq!(
        draw(&mut ui, &snapshot(&mut app, &descriptor, 0.0))
            .quads
            .instances
            .len(),
        2
    );
    sample(
        &mut app,
        3,
        16,
        &[
            ("player.reloadProgress", correlated(1.0)),
            ("player.reloadActive", correlated(1.0)),
        ],
    );
    sample(
        &mut app,
        4,
        18,
        &[
            ("player.reloadProgress", correlated(0.0)),
            ("player.reloadActive", correlated(0.0)),
        ],
    );
    advance(&mut app, 18.0, 3.0 / 60.0);
    let fall = snapshot(&mut app, &descriptor, 0.1);
    assert_eq!(number(&fall, "player.reloadProgress"), 0.0);
    let fade = draw(&mut ui, &fall);
    assert_eq!(
        fade.quads.instances.len(),
        1,
        "the exit image captures the fall's empty fill, not the previous projected progress or Completed endpoint"
    );
    assert!(
        fade.quads.instances[0].color[3] > 0.0,
        "background still exits with its authored fade"
    );
}

#[test]
fn timed_slot_ui_snapshot_does_not_restore_absent_heat_or_cell() {
    let mut app = ramping_client();
    sample(
        &mut app,
        3,
        16,
        &[
            ("player.heat", WireSlotValue::Array(vec![0.0])),
            ("player.cell", WireSlotValue::Array(vec![0.0])),
        ],
    );
    advance(&mut app, 16.0, 1.0 / 60.0);
    let descriptor = tree(bar(json!({"slot":"player.cell"}), json!(128.0)));
    let absent = snapshot(&mut app, &descriptor, 0.0);
    assert!(!absent.slot_values.contains_key("player.heat"));
    assert!(!absent.slot_values.contains_key("player.cell"));
}

#[test]
fn timed_slot_ui_projection_does_not_fire_raw_state_crossings() {
    use postretro_entities::data_descriptors::{CrossingCondition, CrossingDescriptor};

    let mut app = ramping_client();
    let ctx = app.session.as_ref().unwrap().scripting.script_ctx.clone();
    ctx.data_registry
        .borrow_mut()
        .crossings
        .push(CrossingDescriptor {
            slot: Some("player.reloadProgress".into()),
            condition: CrossingCondition::Above { threshold: 0.5 },
            max: 1.0,
            edge: None,
            fire: vec!["reload_halfway".into()],
        });
    app.session.as_mut().unwrap().crossing_detector.initialize(
        &ctx.data_registry.borrow(),
        &ctx.slot_table.borrow(),
        &ctx,
    );
    let detect = |app: &mut App| {
        app.session
            .as_mut()
            .unwrap()
            .crossing_detector
            .detect(&ctx.slot_table.borrow())
    };
    assert!(detect(&mut app).is_empty());
    advance(&mut app, 18.0, 3.0 / 60.0);
    let descriptor = tree(bar(json!({"slot":"player.reloadProgress"}), json!(1.0)));
    assert!(
        number(
            &snapshot(&mut app, &descriptor, 0.0),
            "player.reloadProgress"
        ) > 0.5
    );
    assert!(
        detect(&mut app).is_empty(),
        "crossing the threshold only in the UI target cannot fire a store reaction"
    );
    advance(&mut app, 19.0, 1.0 / 60.0);
    assert!(detect(&mut app).is_empty());
    sample(
        &mut app,
        3,
        20,
        &[("player.reloadProgress", correlated(0.75))],
    );
    advance(&mut app, 20.0, 1.0 / 60.0);
    assert_eq!(detect(&mut app), vec!["reload_halfway".to_string()]);
    assert!(
        detect(&mut app).is_empty(),
        "the accepted host crossing fires only once"
    );
}

fn raw_weapon(app: &App) -> crate::weapon::ReplicatedWeaponProjection {
    crate::netcode::client_weapon_projection(
        app.session
            .as_ref()
            .and_then(|session| session.net_endpoint.as_ref()),
    )
}

fn resource_weapon(resource: Value) -> postretro_entities::components::weapon::WeaponComponent {
    let descriptor: postretro_foundation::WeaponDescriptor = serde_json::from_value(json!({
        "damage":10.0, "range":64.0, "resolution":"hitscan",
        "primary":{"trigger":"press", "recoveryMs":100.0, "steps":[{"kind":"shot"}]},
        "resource":resource,
    }))
    .unwrap();
    postretro_entities::components::weapon::WeaponComponent::from_descriptor(&descriptor)
}

#[test]
fn timed_slot_ui_full_reload_target_keeps_raw_pull_and_near_full_cancel_silent() {
    use crate::sound_events::{ClientReloadEdges, ProjectedWeapon, ReloadReading};
    use crate::weapon::{ClientPullPresentation, client_pull_presentation};
    use postretro_foundation::ReloadStyle;

    let mut app = test_app();
    app.session.as_mut().unwrap().net_endpoint = endpoint(NetRole::Connect {
        addr: "127.0.0.1:1".parse().unwrap(),
    });
    let weapon_id = postretro_entities::EntityId::from_raw(77);
    let reading = |app: &App| {
        ReloadReading::from_projection(&raw_weapon(app), |_| {
            Some(ProjectedWeapon {
                weapon: weapon_id,
                wielded: true,
                style: ReloadStyle::Magazine,
                capacity: 8,
            })
        })
    };
    let mut projected_edges = ClientReloadEdges::default();
    let mut unprojected_edges = ClientReloadEdges::default();
    advance(&mut app, 6.0, 0.0);
    for (sequence, tick, active, progress) in [
        (0, 6, false, 0.0),
        (1, 8, true, 0.0),
        (2, 10, true, 0.75),
        (3, 12, true, 0.8125),
        (4, 14, true, 0.875),
    ] {
        sample(
            &mut app,
            sequence,
            tick,
            &[
                ("player.reloadProgress", correlated(progress)),
                (
                    "player.reloadActive",
                    correlated(f32::from(u8::from(active))),
                ),
                ("player.ammo", correlated(2.0)),
                ("player.ammoReserve", correlated(24.0)),
            ],
        );
        let raw = raw_weapon(&app);
        let expected = unprojected_edges.observe_reading(reading(&app));
        advance(
            &mut app,
            f64::from(tick),
            if sequence == 0 { 0.0 } else { 2.0 / 60.0 },
        );
        assert_eq!(raw_weapon(&app), raw);
        assert_eq!(
            projected_edges.observe_reading(reading(&app)),
            expected,
            "timed target advancement cannot move a reload sound's frame"
        );
        if sequence == 1 {
            assert_eq!(expected, ["reload_started"]);
        } else {
            assert!(expected.is_empty());
        }
    }
    let raw = raw_weapon(&app);
    advance(&mut app, 20.0, 6.0 / 60.0);
    let descriptor = tree(bar(json!({"slot":"player.reloadProgress"}), json!(1.0)));
    assert_eq!(
        number(
            &snapshot(&mut app, &descriptor, 0.0),
            "player.reloadProgress"
        ),
        1.0
    );
    assert_eq!(raw_weapon(&app), raw);
    let weapon = resource_weapon(json!({
        "kind":"ammo", "type":"bullets.test", "magazine":8,
        "reserve":24, "reloadMs":1000,
    }));
    assert_eq!(
        client_pull_presentation(&weapon, 0, &raw_weapon(&app)),
        ClientPullPresentation::Silent,
        "a full display target does not make a still-reloading magazine fire"
    );
    assert!(projected_edges.observe_reading(reading(&app)).is_empty());
    assert!(unprojected_edges.observe_reading(reading(&app)).is_empty());
    sample(
        &mut app,
        5,
        22,
        &[
            ("player.reloadProgress", correlated(0.0)),
            ("player.reloadActive", correlated(0.0)),
            ("player.ammo", correlated(2.0)),
            ("player.ammoReserve", correlated(24.0)),
        ],
    );
    advance(&mut app, 22.0, 2.0 / 60.0);
    assert!(
        projected_edges.observe_reading(reading(&app)).is_empty(),
        "cancelling near full cannot sound complete because the display reached one"
    );
    assert!(unprojected_edges.observe_reading(reading(&app)).is_empty());
}

#[test]
fn timed_slot_ui_regen_target_does_not_supply_cell_charge_to_dry_fire() {
    use crate::weapon::{ClientPullPresentation, client_pull_presentation};

    let mut app = ramping_client();
    sample(&mut app, 3, 16, &[("player.cell", correlated(1.0))]);
    advance(&mut app, 16.0, 1.0 / 60.0);
    sample(&mut app, 4, 18, &[("player.cell", correlated(2.0))]);
    advance(&mut app, 18.0, 2.0 / 60.0);
    sample(&mut app, 5, 20, &[("player.cell", correlated(3.0))]);
    advance(&mut app, 20.0, 2.0 / 60.0);
    let raw = raw_weapon(&app);
    advance(&mut app, 24.0, 4.0 / 60.0);
    let descriptor = tree(bar(json!({"slot":"player.cell"}), json!(128.0)));
    assert!(number(&snapshot(&mut app, &descriptor, 0.0), "player.cell") >= 4.0);
    assert_eq!(raw_weapon(&app), raw);
    assert_eq!(raw.cell.unwrap().value, Some(3.0));
    let weapon = resource_weapon(
        json!({"kind":"cell", "capacity":40.0, "costPerShot":4.0, "regenPerSecond":8.0}),
    );
    let installed_cell = weapon.cell;
    assert_eq!(
        client_pull_presentation(&weapon, 0, &raw_weapon(&app)),
        ClientPullPresentation::DryFire
    );
    assert_eq!(
        weapon.cell, installed_cell,
        "a display target does not write the installed cell state"
    );
}

#[test]
fn timed_slot_ui_cooling_target_keeps_overheat_cue_frames_on_raw_latch() {
    use crate::sound_events::{ClientOverheatEdge, OverheatReading, ProjectedHeatWeapon};

    let mut app = ramping_client();
    let weapon_id = postretro_entities::EntityId::from_raw(77);
    let reading = |app: &App| {
        OverheatReading::from_projection(&raw_weapon(app), |_| {
            Some(ProjectedHeatWeapon {
                weapon: weapon_id,
                wielded: true,
            })
        })
    };
    let mut projected_edges = ClientOverheatEdge::default();
    let mut unprojected_edges = ClientOverheatEdge::default();
    for (sequence, tick, heat, latched) in [
        (3, 16, 64.0, false),
        (4, 18, 128.0, true),
        (5, 20, 120.0, true),
        (6, 22, 112.0, true),
    ] {
        sample(
            &mut app,
            sequence,
            tick,
            &[
                ("player.heat", correlated(heat)),
                (
                    "player.overheated",
                    correlated(f32::from(u8::from(latched))),
                ),
            ],
        );
        let raw = raw_weapon(&app);
        let expected = unprojected_edges.observe_reading(reading(&app));
        advance(&mut app, f64::from(tick), 2.0 / 60.0);
        assert_eq!(raw_weapon(&app), raw);
        assert_eq!(projected_edges.observe_reading(reading(&app)), expected);
        assert_eq!(
            expected,
            sequence == 4,
            "only the host latch's rising sample sounds overheat"
        );
    }
    advance(&mut app, 24.0, 2.0 / 60.0);
    let descriptor = tree(bar(json!({"slot":"player.heat"}), json!(128.0)));
    assert!(number(&snapshot(&mut app, &descriptor, 0.0), "player.heat") < 112.0);
    assert!(!projected_edges.observe_reading(reading(&app)));
    assert!(!unprojected_edges.observe_reading(reading(&app)));
    sample(
        &mut app,
        7,
        26,
        &[
            ("player.heat", correlated(0.0)),
            ("player.overheated", correlated(0.0)),
        ],
    );
    advance(&mut app, 26.0, 2.0 / 60.0);
    assert!(!projected_edges.observe_reading(reading(&app)));
    assert!(!unprojected_edges.observe_reading(reading(&app)));
}

#[test]
fn timed_slot_ui_cooldown_target_does_not_enter_weapon_reconciliation() {
    use postretro_entities::Transform;
    use postretro_entities::components::{inventory::Inventory, weapon::WeaponComponent};

    let mut app = ramping_client();
    let descriptor = tree(bar(json!({"slot":"player.weaponCooldownMs"}), json!(128.0)));
    let shown = number(
        &snapshot(&mut app, &descriptor, 0.0),
        "player.weaponCooldownMs",
    );
    assert!(shown < 64.0);
    let ctx = app.session.as_ref().unwrap().scripting.script_ctx.clone();
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    registry
        .set_component(
            pawn,
            postretro_foundation::PlayerMovementComponent::from_descriptor(
                &crate::tests::minimal_player_descriptor(),
            ),
        )
        .unwrap();
    let weapon_id = registry.spawn(Transform::default());
    registry
        .set_component(
            weapon_id,
            crate::weapon::test_fixtures::weapon_component(
                postretro_foundation::ActivationTrigger::Press,
                100.0,
            ),
        )
        .unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(weapon_id);
    registry.set_component(pawn, inventory).unwrap();
    assert!(crate::reconcile_client_weapon_cooldown_from_slot_table(
        &mut crate::weapon::ClientPredictedShots::new(),
        &mut registry,
        &ctx.slot_table.borrow(),
        Some(0),
    ));
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .cooldown_remaining_ms,
        64.0,
        "cooldown reconciliation uses the committed store, not the UI display target"
    );
}
