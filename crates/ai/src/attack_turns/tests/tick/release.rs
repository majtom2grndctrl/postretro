// Turns ending outside their own rules: the holder leaving the AI set, and a
// serialize-and-restore.

use super::*;
use postretro_entities::components::inventory::Inventory;

/// A marked standing attacker: it requests while `strike` is on its path,
/// ready to fire or not, so a returning enemy asks again.
fn marked_striker() -> BehaviorGraphDescriptor {
    graph(
        "strike",
        vec![
            ("strike", marked(striking("attack"), "attack")),
            ("away", idle()),
        ],
        vec![],
        vec![("attack", contact(1.0, 100_000.0))],
    )
}

#[derive(Clone, Copy, Debug)]
enum Exit {
    Quiescence,
    InventoryOwnership,
    KilledEarlierInPass,
    AggroClosing,
    GraphReseat,
}

// B36 / P5: no turn leaks across any way a holder leaves the AI set. It stops
// counting from the tick it leaves, a waiter is granted no later than the
// next tick, and a returning enemy re-requests with a fresh clock.
#[test]
fn turn_releases_on_every_ai_set_exit() {
    for exit in [
        Exit::Quiescence,
        Exit::InventoryOwnership,
        Exit::KilledEarlierInPass,
        Exit::AggroClosing,
        Exit::GraphReseat,
    ] {
        let graph = marked_striker();
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        set_limits(&mut runtime, &[1]);
        let pawn = spawn_player(&mut reg, player_spot());
        let holder = spawn_at(&mut reg, 0, &graph, "strike");
        let waiter = spawn_at(&mut reg, 3, &graph, "strike");

        tick(&mut reg, &mut runtime);
        assert_eq!(held(&reg, holder), vec!["attack"], "{exit:?}");
        assert_eq!(wait_since(&reg, waiter), Some(1), "{exit:?}");

        match exit {
            Exit::Quiescence => set_hp(&mut reg, holder, 0.0),
            Exit::InventoryOwnership => {
                let mut inventory = Inventory::default();
                inventory.wieldables[9] = Some(holder);
                reg.set_component(pawn, inventory).unwrap();
            }
            Exit::KilledEarlierInPass => {
                // A player-faction brain beside the holder kills it in the
                // tick's apply pass, after this tick's arbitration counted it.
                let mut lethal = striker(100_000.0);
                lethal.attacks.get_mut("attack").unwrap().damage = Some(100.0);
                let killer = spawn_enemy(
                    &mut reg,
                    attacker_spot(0) - Vec3::X,
                    authored_brain(&lethal, "strike"),
                    50.0,
                );
                reg.entity_state_mut(killer).unwrap().set(
                    FACTION_STATE_FIELD,
                    postretro_entities::PLAYER_FACTION_INDEX,
                );
            }
            Exit::AggroClosing => set_enemy_aggro_armed(&mut reg, holder, false),
            Exit::GraphReseat => {
                let mut brain = reg.get_component::<BrainComponent>(holder).unwrap().clone();
                brain.graph = std::sync::Arc::new((*brain.graph).clone());
                reg.set_component(holder, brain).unwrap();
            }
        }
        tick(&mut reg, &mut runtime);
        if matches!(exit, Exit::KilledEarlierInPass) {
            assert_eq!(
                reg.get_component::<HealthComponent>(holder)
                    .unwrap()
                    .current,
                0.0,
                "fixture: the holder dies in the pass"
            );
            assert!(held(&reg, waiter).is_empty(), "it still counted this tick");
            tick(&mut reg, &mut runtime);
        }
        let granted_at = if matches!(exit, Exit::KilledEarlierInPass) {
            3
        } else {
            2
        };
        assert_eq!(
            held(&reg, waiter),
            vec!["attack"],
            "{exit:?}: the waiter is granted"
        );

        // The holder returns.
        match exit {
            Exit::Quiescence | Exit::KilledEarlierInPass => set_hp(&mut reg, holder, 50.0),
            Exit::InventoryOwnership => reg.set_component(pawn, Inventory::default()).unwrap(),
            Exit::AggroClosing => set_enemy_aggro_armed(&mut reg, holder, true),
            Exit::GraphReseat => {}
        }
        let returned_at = if matches!(exit, Exit::GraphReseat) {
            granted_at
        } else {
            tick(&mut reg, &mut runtime);
            granted_at + 1
        };
        assert!(
            held(&reg, holder).is_empty(),
            "{exit:?}: nothing leaked back"
        );
        assert_eq!(
            requested(&reg, holder).as_deref(),
            Some("attack"),
            "{exit:?}"
        );
        assert_eq!(
            wait_since(&reg, holder),
            Some(returned_at),
            "{exit:?}: a returning enemy re-requests on a fresh clock"
        );
        assert_eq!(held(&reg, waiter), vec!["attack"], "{exit:?}");
    }
}

// B38 / P11: a serialize-and-restore drops every turn and wait clock; the
// first restored tick arbitrates from scratch.
#[test]
fn restore_drops_turns_and_wait_clocks() {
    let graph = marked_striker();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let pawn = spawn_player(&mut reg, player_spot());
    // The earlier slot asks second, so only a restored-from-scratch line
    // would grant it first.
    let late = spawn_at(&mut reg, 0, &graph, "away");
    let holder = spawn_at(&mut reg, 1, &graph, "strike");
    tick(&mut reg, &mut runtime);
    enter(&mut reg, late, "strike");
    for _ in 0..3 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(held(&reg, holder), vec!["attack"]);
    assert_eq!(wait_since(&reg, late), Some(2));

    let kinds = [
        ComponentKind::Transform,
        ComponentKind::PlayerMovement,
        ComponentKind::Mesh,
        ComponentKind::Health,
        ComponentKind::Agent,
        ComponentKind::Brain,
        ComponentKind::EntityState,
    ];
    let mut restored = EntityRegistry::new();
    for id in [pawn, late, holder] {
        let saved: Vec<serde_json::Value> = kinds
            .iter()
            .filter_map(|&kind| {
                reg.iter_with_kind(kind)
                    .find(|(entity, _)| *entity == id)
                    .map(|(_, value)| serde_json::to_value(value).expect("component serializes"))
            })
            .collect();
        let restored_id = restored.spawn(Transform::default());
        assert_eq!(restored_id, id, "fixture: restore keeps entity ids");
        for value in saved {
            let value: ComponentValue =
                serde_json::from_value(value).expect("component deserializes");
            restored.set_component_value(restored_id, value).unwrap();
        }
    }
    for enemy in [late, holder] {
        assert_eq!(
            turns(&restored, enemy),
            AttackTurnState::default(),
            "no turn, request or clock survives"
        );
    }

    let mut restored_runtime = AiRuntime::new();
    set_limits(&mut restored_runtime, &[1]);
    tick(&mut restored, &mut restored_runtime);
    assert_eq!(
        held(&restored, late),
        vec!["attack"],
        "both ask fresh on the first tick, so the earlier slot wins"
    );
    assert!(held(&restored, holder).is_empty());
    assert_eq!(wait_since(&restored, holder), Some(1));
}
