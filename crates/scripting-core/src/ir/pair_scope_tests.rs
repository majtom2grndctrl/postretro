// The interaction-pair read table: runtime side seeding and resolution for
// both roles, missing side values, and the value-free twin's refusals at
// AC1's key path.
// See: context/lib/scripting.md §11 (interaction-pair scope)

use std::collections::BTreeMap;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityTypeDescriptor, HealthDescriptor, LifeStatesDescriptor, Transform};
use postretro_foundation::{
    Assistable, LifeStateProfile, NumberOrIr, PlayerMovementComponent, PlayerMovementDescriptor,
    Seat,
};
use serde_json::json;

use super::*;
use crate::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, bind, eval_value_checked};
use crate::ir_pair_validation::{PairValidationScope, validate_assistable_expressions};
use crate::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    StoreDeclarationSet,
};

const ALIVE: u8 = 0;
const DOWN: u8 = 1;

fn schema(slot_type: SlotType, default: SlotValue, per_owner: bool) -> SlotSchema {
    SlotSchema {
        slot_type,
        default: Some(default),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: if per_owner {
            ReplicationScope::OwnerPrivatePlayer
        } else {
            ReplicationScope::None
        },
        per_owner,
        accumulate: None,
    }
}

/// `stats.heal` (per-owner number), `stats.medic` (per-owner bool, no
/// default), `world.bonus` (global number), life states `alive` and `down`.
fn pair_ctx() -> ScriptCtx {
    let ctx = ScriptCtx::new();
    ctx.data_registry.borrow_mut().life_state_names = vec!["alive".into(), "down".into()];
    let mut table = ctx.slot_table.borrow_mut();
    let mut medic = schema(SlotType::Boolean, SlotValue::Boolean(false), true);
    medic.default = None;
    table
        .insert_namespace(
            "stats",
            vec![
                (
                    "heal".into(),
                    SlotRecord::new(schema(SlotType::Number, SlotValue::Number(0.0), true)),
                ),
                ("medic".into(), SlotRecord::new(medic)),
            ],
        )
        .unwrap();
    table
        .insert_namespace(
            "world",
            vec![(
                "bonus".into(),
                SlotRecord::new(schema(SlotType::Number, SlotValue::Number(2.0), false)),
            )],
        )
        .unwrap();
    drop(table);
    ctx
}

fn movement(state: u8) -> PlayerMovementComponent {
    let descriptor: PlayerMovementDescriptor = serde_json::from_value(json!({
        "capsule": { "radius": 0.4, "half_height": 0.8, "eye_height": 0.5 },
        "ground": { "speed": { "walk": 7.0, "run": 11.0, "crouch": 3.0 },
            "accel": 10.0, "step_height": 0.3, "max_slope": 45.0,
            "friction": 8.0, "stop_speed": 2.5 },
        "air": { "forward_steer": 0.0, "accel": 0.7, "max_control_speed": 0.5,
            "bunny_hop": false, "jumps": 0, "jump_velocity": 5.5, "jump_ceiling": 0.0 },
        "fall": { "terminal_velocity": 40.0 },
        "stuck_stop_enabled": true, "stuck_stop_threshold": 0.001
    }))
    .unwrap();
    let mut movement = PlayerMovementComponent::from_descriptor(&descriptor);
    movement.life_state = state;
    movement
}

/// A pawn with health, a life state and optionally a seat holding `heal`.
fn pawn(ctx: &ScriptCtx, health: Option<f32>, state: u8, seat: Option<(u16, f32)>) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    registry.set_component(pawn, movement(state)).unwrap();
    if let Some(current) = health {
        let mut component = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        component.current = current;
        registry.set_component(pawn, component).unwrap();
    }
    if let Some((seat, heal)) = seat {
        registry.bind_pawn_seat(pawn, Seat(seat));
        ctx.slot_table
            .borrow_mut()
            .get_mut("stats.heal")
            .unwrap()
            .set_per_seat_value(Seat(seat), SlotValue::Number(heal));
    }
    pawn
}

fn input(name: &str, owner: Option<&str>) -> IrNode {
    IrNode::Input {
        name: name.into(),
        owner: owner.map(str::to_string),
    }
}

fn baked(root: IrNode) -> BakedIr {
    BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root,
    }
}

const HELPER: &str = "@assist.helper";
const TARGET: &str = "@assist.target";

#[test]
fn pair_scope_each_read_row_binds_and_resolves_for_both_roles() {
    let ctx = pair_ctx();
    let a = pawn(&ctx, Some(80.0), ALIVE, Some((0, 7.0)));
    let b = pawn(&ctx, Some(30.0), DOWN, Some((1, 11.0)));
    let scope = InteractionPairScope::new(ctx.clone(), ASSIST_PAIR_ROLES);

    // (read, value for actor A, value for subject B) per role.
    let rows = [
        (
            "player.health",
            IrValue::Number(80.0),
            IrValue::Number(30.0),
        ),
        ("stats.heal", IrValue::Number(7.0), IrValue::Number(11.0)),
        (
            "player.inLifeState:down",
            IrValue::Bool(false),
            IrValue::Bool(true),
        ),
    ];
    let mut bound = Vec::new();
    for (name, _, _) in rows {
        for role in [HELPER, TARGET] {
            scope.begin_expression();
            let program = bind(&baked(input(name, Some(role))), &scope)
                .unwrap_or_else(|error| panic!("`{name}` under {role}: {error}"));
            bound.push((name, role, program, scope.finish_expression()));
        }
    }
    scope.begin_expression();
    let global = bind(&baked(input("world.bonus", None)), &scope).expect("global read binds");
    assert!(
        scope.finish_expression().is_empty(),
        "a global read is not a side input"
    );

    scope.seed_sides(
        &ctx.registry.borrow(),
        Some(&ctx.slot_table.borrow()),
        &[a, b],
    );
    for (actor, subject) in [(a, b), (b, a)] {
        assert!(scope.select(actor, subject));
        for ((name, role, program, inputs), _) in bound.iter().zip(0..) {
            let row = rows.iter().find(|row| row.0 == *name).unwrap();
            let reads_a = (*role == HELPER) == (actor == a);
            let expected = if reads_a { row.1 } else { row.2 };
            assert!(scope.observes(inputs), "`{name}` under {role}");
            assert_eq!(
                eval_value_checked(program, &scope),
                Ok(expected),
                "`{name}` under {role}"
            );
        }
        assert_eq!(
            eval_value_checked(&global, &scope),
            Ok(IrValue::Number(2.0))
        );
    }
}

#[test]
fn pair_scope_single_player_pawn_reads_its_per_owner_value_at_its_seat() {
    let ctx = pair_ctx();
    // The single-player pawn holds Seat(0), as the session binds it.
    let local = pawn(&ctx, Some(100.0), DOWN, Some((0, 9.0)));
    let scope = InteractionPairScope::new(ctx.clone(), ASSIST_PAIR_ROLES);
    scope.begin_expression();
    let program = bind(&baked(input("stats.heal", Some(HELPER))), &scope).unwrap();
    let inputs = scope.finish_expression();
    scope.seed_sides(
        &ctx.registry.borrow(),
        Some(&ctx.slot_table.borrow()),
        &[local],
    );
    assert!(scope.select(local, local));
    assert!(scope.observes(&inputs));
    assert_eq!(
        eval_value_checked(&program, &scope),
        Ok(IrValue::Number(9.0))
    );
}

#[test]
fn pair_scope_missing_side_value_is_unobserved_bool_included() {
    let ctx = pair_ctx();
    let seated = pawn(&ctx, Some(50.0), ALIVE, Some((0, 1.0)));
    // No seat (per-owner reads missing) and no health component.
    let bare = pawn(&ctx, None, DOWN, None);
    let scope = InteractionPairScope::new(ctx.clone(), ASSIST_PAIR_ROLES);
    let mut expressions = Vec::new();
    for (name, role) in [
        ("stats.medic", TARGET),       // per-owner Bool with no seat
        ("stats.heal", TARGET),        // per-owner Number with no seat
        ("player.health", TARGET),     // no health component
        ("player.overheated", HELPER), // Bool engine slot on an unarmed pawn
    ] {
        scope.begin_expression();
        bind(&baked(input(name, Some(role))), &scope).unwrap();
        expressions.push((name, scope.finish_expression()));
    }
    scope.seed_sides(
        &ctx.registry.borrow(),
        Some(&ctx.slot_table.borrow()),
        &[seated, bare],
    );
    assert!(scope.select(seated, bare));
    for (name, inputs) in &expressions {
        assert!(!scope.observes(inputs), "`{name}` must read missing");
    }
    // An unseeded pawn selects nothing, and nothing observes.
    let unseeded = pawn(&ctx, Some(1.0), ALIVE, None);
    assert!(!scope.select(seated, unseeded));
    assert!(!scope.observes(&expressions[2].1));
}

/// Every refused read, with the substring its diagnostic carries.
fn refused_rows() -> Vec<(IrNode, &'static str)> {
    vec![
        (input("stats.heal", Some("@player")), "does not know"),
        (input("stats.heal", Some("@impact.source")), "does not know"),
        (input("@state.marked", None), "per-entity state"),
        (input("@state.marked", Some(HELPER)), "per-entity state"),
        (
            input("@pair.distance", None),
            "`@pair.` fact namespace is reserved",
        ),
        (
            input("@pair.distance", Some(TARGET)),
            "`@pair.` fact namespace is reserved",
        ),
        (input("@impact.amount", None), "dispatch input"),
        (
            input("player.spread", None),
            "each machine publishes for its own player",
        ),
        (
            input("player.spread", Some(HELPER)),
            "not a per-player value",
        ),
        (
            input("player.assistProgress", Some(HELPER)),
            "depends on this pair evaluation",
        ),
        (
            input("player.assistReceivedProgress", Some(TARGET)),
            "depends on this pair evaluation",
        ),
        (
            input("player.health", None),
            "use `ref.byPlayer(assist.helper)` or `ref.byPlayer(assist.target)` \
             (Luau: `ref:byPlayer(assist.helper)` or `ref:byPlayer(assist.target)`)",
        ),
        (
            input("stats.heal", None),
            "use `ref.byPlayer(assist.helper)` or `ref.byPlayer(assist.target)` \
             (Luau: `ref:byPlayer(assist.helper)` or `ref:byPlayer(assist.target)`)",
        ),
        (
            input("player.inLifeState:down", None),
            "use `assist.helper.inLifeState(name)` or `assist.target.inLifeState(name)` \
             (Luau: `assist.helper:inLifeState(name)` or `assist.target:inLifeState(name)`)",
        ),
        (
            input("player.inLifeState:gone", Some(TARGET)),
            "life state `gone`",
        ),
        (input("nope.value", None), "unknown input `nope.value`"),
        (input("world.bonus", Some(HELPER)), "not a per-player value"),
    ]
}

#[test]
fn pair_scope_rejected_rows_fail_the_runtime_bind_with_their_reason() {
    let ctx = pair_ctx();
    let scope = InteractionPairScope::new(ctx, ASSIST_PAIR_ROLES);
    for (node, reason) in refused_rows() {
        scope.begin_expression();
        assert!(bind(&baked(node.clone()), &scope).is_err(), "{node:?}");
        let error = scope.take_bind_error().expect("a refused read records why");
        assert!(error.contains(reason), "{node:?}: {error}");
    }
}

fn descriptor(radius: NumberOrIr, hold_ms: NumberOrIr) -> EntityTypeDescriptor {
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
                        assistable: Some(Assistable { radius, hold_ms }),
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

/// The incoming declarations a mod attempt carries: `stats.heal` per-owner
/// and `world.bonus` global, absent from the live table.
fn incoming_declarations() -> StoreDeclarationSet {
    let mut set = StoreDeclarationSet::default();
    set.add(crate::slot_table::StoreDeclaration {
        namespace: "stats".into(),
        records: vec![
            (
                "heal".into(),
                SlotRecord::new(schema(SlotType::Number, SlotValue::Number(0.0), true)),
            ),
            (
                "medic".into(),
                SlotRecord::new(schema(SlotType::Boolean, SlotValue::Boolean(false), true)),
            ),
        ],
    })
    .unwrap();
    set.add(crate::slot_table::StoreDeclaration {
        namespace: "world".into(),
        records: vec![(
            "bonus".into(),
            SlotRecord::new(schema(SlotType::Number, SlotValue::Number(2.0), false)),
        )],
    })
    .unwrap();
    set
}

#[test]
fn pair_validation_rejected_rows_fail_with_ac1_key_path() {
    let live = crate::slot_table::SlotTable::new();
    let declarations = incoming_declarations();
    let deep = (0..40).fold(
        IrNode::Const {
            value: IrValue::Number(1.0),
        },
        |node, _| IrNode::Add {
            a: Box::new(node),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        },
    );
    let mut rows = refused_rows();
    rows.push((
        IrNode::Lt {
            a: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(2.0),
            }),
        },
        "must produce a number",
    ));
    rows.push((deep, "depth 32 or 256 nodes"));
    for (node, reason) in rows {
        for (field, entity) in [
            (
                "radius",
                descriptor(NumberOrIr::Ir(node.clone()), NumberOrIr::Literal(3000.0)),
            ),
            (
                "holdMs",
                descriptor(NumberOrIr::Literal(1.5), NumberOrIr::Ir(node.clone())),
            ),
        ] {
            let error = validate_assistable_expressions(&[entity], &declarations, &live)
                .expect_err("refused read must reject the load");
            assert!(
                error.contains(&format!(
                    "descriptor `player` `components.lifeStates.states.down.assistable.{field}`"
                )),
                "{error}"
            );
            assert!(error.contains(reason), "{node:?}: {error}");
        }
    }
}

#[test]
fn pair_validation_accepts_every_read_row_from_the_incoming_schemas() {
    let live = crate::slot_table::SlotTable::new();
    let declarations = incoming_declarations();
    let heal = input("stats.heal", Some(HELPER));
    let rows = [
        input("player.health", Some(TARGET)),
        heal.clone(),
        input("world.bonus", None),
        IrNode::Select {
            cond: Box::new(input("player.inLifeState:down", Some(TARGET))),
            a: Box::new(IrNode::Const {
                value: IrValue::Number(3.0),
            }),
            b: Box::new(input("stats.heal", Some(TARGET))),
        },
        IrNode::Select {
            cond: Box::new(input("stats.medic", Some(HELPER))),
            a: Box::new(heal),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        },
    ];
    for node in rows {
        let entity = descriptor(NumberOrIr::Ir(node.clone()), NumberOrIr::Ir(node.clone()));
        validate_assistable_expressions(&[entity], &declarations, &live)
            .unwrap_or_else(|error| panic!("{node:?}: {error}"));
    }
    // The same per-owner name with no declaration is unknown.
    let names = crate::ir_pair_validation::mod_life_state_names(&[]);
    let empty = StoreDeclarationSet::default();
    let scope = PairValidationScope::new(ASSIST_PAIR_ROLES, &empty, &live, &names);
    assert!(
        scope
            .validate_expression(&input("stats.heal", Some(HELPER)))
            .unwrap_err()
            .contains("unknown input `stats.heal`")
    );
}

#[test]
fn owner_token_list_names_the_dispatch_tokens_it_mirrors() {
    use postretro_foundation::owner_tokens::BY_PLAYER_OWNER_TOKENS;
    assert_eq!(
        BY_PLAYER_OWNER_TOKENS,
        [
            crate::player_event_scope::EVENT_PLAYER_TOKEN,
            crate::components::health::IMPACT_SOURCE_TOKEN,
            ASSIST_PAIR_ROLES.actor_token,
            ASSIST_PAIR_ROLES.subject_token,
        ]
    );
}
