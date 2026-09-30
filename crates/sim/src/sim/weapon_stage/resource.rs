// Heat and cell: the per-tick resource update, their fire-gate terms, and per-shot cost.
// See: context/lib/entity_model.md §Components (Weapon resources)

use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityRegistry};

use crate::weapon::WeaponFireAuthorization;

/// Advance every heat and cell instance by one fixed tick: wielded, holstered
/// and unowned alike, so swapping away to cool a gun is a real tactic. Runs on
/// the authoritative path only, once per tick, before any fire gate; connected
/// clients read the replicated values instead. `cooldown_remaining_ms` is not
/// touched here — inactive instances keep freezing it.
pub(in crate::sim) fn tick_weapon_resources(registry: &mut EntityRegistry, tick_dt: f32) {
    let dt_ms = tick_dt.max(0.0) * 1000.0;
    registry.for_each_with_kind_mut(ComponentKind::Weapon, |_, value| {
        if let ComponentValue::Weapon(weapon) = value {
            advance_weapon_resource(weapon, dt_ms);
        }
    });
}

/// Idle-then-apply, the `tick_bloom` shape: the whole `dt` applies once the
/// delay is met, with no partial-tick splitting.
fn advance_weapon_resource(weapon: &mut WeaponComponent, dt_ms: f32) {
    let stats = weapon.effective();
    let (heat_stats, cell_stats) = (stats.heat, stats.cell);
    if let (Some(heat), Some(stats)) = (weapon.heat.as_mut(), heat_stats) {
        heat.idle_ms += dt_ms;
        // Lockout ignores the delay: the punish window is overheatAt / coolPerSecond.
        if heat.overheated || heat.idle_ms >= stats.cool_delay_ms {
            heat.heat = (heat.heat - stats.cool_per_second * (dt_ms / 1000.0)).max(0.0);
            if heat.heat == 0.0 {
                heat.overheated = false;
            }
        }
    }
    if let (Some(cell), Some(stats)) = (weapon.cell.as_mut(), cell_stats) {
        cell.idle_ms += dt_ms;
        if cell.idle_ms >= stats.regen_delay_ms {
            cell.charge =
                (cell.charge + stats.regen_per_second * (dt_ms / 1000.0)).min(stats.capacity);
        }
    }
}

/// The heat/cell term of the state-blind fire verdict, beside the magazine
/// check. An overheat refuses silently (one cue per latch, not a click per
/// pull); a short cell dry-fires exactly as an empty magazine does.
pub(super) fn resource_fire_verdict(weapon: &WeaponComponent) -> Option<WeaponFireAuthorization> {
    if weapon.heat.is_some_and(|heat| heat.overheated) {
        return Some(WeaponFireAuthorization::Rejected);
    }
    if let (Some(cell), Some(stats)) = (weapon.cell, weapon.effective().cell)
        && cell.charge < stats.cost_per_shot
    {
        return Some(WeaponFireAuthorization::Empty);
    }
    None
}

/// Charge an accepted shot to its heat or cell. The shot that reaches
/// `overheat_at` still fires; it latches the weapon afterward.
pub(super) fn spend_shot_resource(weapon: &mut WeaponComponent) {
    let stats = weapon.effective();
    let (heat_stats, cell_stats) = (stats.heat, stats.cell);
    if let (Some(heat), Some(stats)) = (weapon.heat.as_mut(), heat_stats) {
        heat.heat += stats.heat_per_shot;
        if heat.heat >= stats.overheat_at {
            heat.heat = stats.overheat_at;
            heat.overheated = true;
        }
        heat.idle_ms = 0.0;
    }
    if let (Some(cell), Some(stats)) = (weapon.cell.as_mut(), cell_stats) {
        cell.charge = (cell.charge - stats.cost_per_shot).max(0.0);
        cell.idle_ms = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::components::weapon::WeaponComponent;
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::data_descriptors::WeaponDescriptor;
    use postretro_entities::{EntityId, EntityRegistry, Transform};

    use crate::collision::CollisionWorld;
    use crate::scripting_systems::hit_zones::HitZoneStore;
    use crate::sim::tests::{
        remote_command, run_local_only_tick, run_remote_only_tick, sim_command, trigger_movement,
    };
    use crate::sim::{TickEvents, simulate_client_wieldable_tick};
    use crate::weapon::FireButtonState;

    // Binary-exact tick so per-tick rates accumulate without float residue.
    const DT: f32 = 0.125;

    fn weapon(fire_mode: &str, resource: serde_json::Value) -> WeaponComponent {
        let mut descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 1.0,
            "range": 64.0,
            "fireRateMs": 100.0,
            "fireMode": fire_mode,
            "resolution": "hitscan",
        }))
        .unwrap();
        descriptor.resource =
            (!resource.is_null()).then(|| serde_json::from_value(resource).unwrap());
        WeaponComponent::from_descriptor(&descriptor.validate().unwrap())
    }

    fn heat(heat_per_shot: f32, cool_per_second: f32, cool_delay_ms: f32) -> serde_json::Value {
        serde_json::json!({
            "kind": "heat",
            "heatPerShot": heat_per_shot,
            "overheatAt": 100.0,
            "coolPerSecond": cool_per_second,
            "coolDelayMs": cool_delay_ms,
        })
    }

    fn cell(capacity: f32, cost: f32, regen: f32, regen_delay_ms: f32) -> serde_json::Value {
        serde_json::json!({
            "kind": "cell",
            "capacity": capacity,
            "costPerShot": cost,
            "regenPerSecond": regen,
            "regenDelayMs": regen_delay_ms,
        })
    }

    /// A local player pawn holding `weapons` in slots 0.., slot 0 active.
    struct Loadout {
        registry: Rc<RefCell<EntityRegistry>>,
        pawn: EntityId,
        slots: Vec<EntityId>,
    }

    impl Loadout {
        fn new(weapons: Vec<WeaponComponent>) -> Self {
            let registry = Rc::new(RefCell::new(EntityRegistry::new()));
            let (pawn, slots) = {
                let mut registry = registry.borrow_mut();
                let pawn = registry.spawn(Transform::default());
                registry.set_component(pawn, trigger_movement()).unwrap();
                registry.mark_local_player_pawn(pawn).unwrap();
                let mut inventory = Inventory::default();
                let mut slots = Vec::new();
                for (slot, component) in weapons.into_iter().enumerate() {
                    let id = registry.spawn(Transform::default());
                    registry.set_component(id, component).unwrap();
                    inventory.wieldables[slot] = Some(id);
                    slots.push(id);
                }
                registry.set_component(pawn, inventory).unwrap();
                (pawn, slots)
            };
            Self {
                registry,
                pawn,
                slots,
            }
        }

        fn tick(&self, fire: bool, select_slot: Option<usize>) -> TickEvents {
            let mut command = sim_command(fire, false);
            command.select_slot = select_slot;
            run_local_only_tick(self.registry.clone(), self.slots[0], &command, DT)
        }

        fn weapon(&self, slot: usize) -> WeaponComponent {
            self.registry
                .borrow()
                .get_component::<WeaponComponent>(self.slots[slot])
                .unwrap()
                .clone()
        }

        fn edit(&self, slot: usize, edit: impl FnOnce(&mut WeaponComponent)) {
            let mut component = self.weapon(slot);
            edit(&mut component);
            self.registry
                .borrow_mut()
                .set_component(self.slots[slot], component)
                .unwrap();
        }

        fn active_slot(&self) -> usize {
            self.registry
                .borrow()
                .get_component::<Inventory>(self.pawn)
                .unwrap()
                .active_slot
        }
    }

    fn addresses(events: &TickEvents) -> Vec<&'static str> {
        events.weapon.iter().map(|event| event.address).collect()
    }

    fn live_heat(loadout: &Loadout, slot: usize) -> (f32, bool) {
        let heat = loadout.weapon(slot).heat.unwrap();
        (heat.heat, heat.overheated)
    }

    #[test]
    fn heat_crossing_shot_fires_then_latches_and_emits_overheat_once() {
        let loadout = Loadout::new(vec![weapon("auto", heat(40.0, 40.0, 250.0))]);

        assert_eq!(addresses(&loadout.tick(true, None)), vec!["activate"]);
        assert_eq!(live_heat(&loadout, 0), (40.0, false));
        assert_eq!(addresses(&loadout.tick(true, None)), vec!["activate"]);
        assert_eq!(live_heat(&loadout, 0), (80.0, false));

        // 80 + 40 overshoots 100: the shot still fires, then the weapon latches.
        let crossing = addresses(&loadout.tick(true, None));
        assert_eq!(crossing, vec!["activate", "overheat"]);
        assert_eq!(live_heat(&loadout, 0), (100.0, true));
    }

    #[test]
    fn heat_lockout_pulls_are_silent_never_dry_fire() {
        for fire_mode in ["auto", "semi"] {
            let loadout = Loadout::new(vec![weapon(fire_mode, heat(40.0, 8.0, 0.0))]);
            loadout.edit(0, |weapon| {
                let heat = weapon.heat.as_mut().unwrap();
                heat.heat = 100.0;
                heat.overheated = true;
            });
            for tick in 0..6 {
                // Alternate press/release so a semi weapon sees fresh presses.
                let events = loadout.tick(tick % 2 == 0, None);
                assert!(
                    addresses(&events).is_empty(),
                    "{fire_mode} tick {tick}: {:?}",
                    addresses(&events)
                );
                assert!(loadout.weapon(0).heat.unwrap().overheated);
            }
            assert!(
                loadout.weapon(0).cooldown_remaining_ms <= 0.0,
                "{fire_mode}: a silent refusal never re-arms the cooldown"
            );
        }
    }

    #[test]
    fn heat_lockout_cools_through_the_delay_and_clears_only_at_zero() {
        // A 10 s cool delay would hold ordinary cooling for 80 ticks; lockout ignores it.
        let loadout = Loadout::new(vec![weapon("auto", heat(40.0, 40.0, 10_000.0))]);
        loadout.edit(0, |weapon| {
            let heat = weapon.heat.as_mut().unwrap();
            heat.heat = 100.0;
            heat.overheated = true;
        });

        // 40/s at 125 ms is 5 heat per tick: 19 ticks leave 5 heat and the latch.
        for _ in 0..19 {
            assert!(addresses(&loadout.tick(true, None)).is_empty());
        }
        assert_eq!(live_heat(&loadout, 0), (5.0, true));

        // The 20th update reaches exactly 0, clears the latch, and the same tick's
        // gate accepts the held trigger.
        assert_eq!(addresses(&loadout.tick(true, None)), vec!["activate"]);
        assert_eq!(live_heat(&loadout, 0), (40.0, false));

        // Unlatched, ordinary cooling waits out the delay again.
        for _ in 0..10 {
            let _ = loadout.tick(false, None);
        }
        assert_eq!(live_heat(&loadout, 0), (40.0, false));
    }

    #[test]
    fn heat_cools_only_after_the_delay_when_not_overheated() {
        let loadout = Loadout::new(vec![weapon("semi", heat(40.0, 40.0, 250.0))]);
        let _ = loadout.tick(true, None);
        assert_eq!(live_heat(&loadout, 0), (40.0, false));
        // idle 125 ms: held.
        let _ = loadout.tick(false, None);
        assert_eq!(live_heat(&loadout, 0), (40.0, false));
        // idle 250 ms meets the delay: the whole tick applies.
        let _ = loadout.tick(false, None);
        assert_eq!(live_heat(&loadout, 0), (35.0, false));
    }

    #[test]
    fn heat_updates_exactly_once_per_simulated_tick() {
        let loadout = Loadout::new(vec![weapon("auto", heat(40.0, 40.0, 0.0))]);
        loadout.edit(0, |weapon| weapon.heat.as_mut().unwrap().heat = 50.0);
        let _ = loadout.tick(false, None);
        assert_eq!(live_heat(&loadout, 0), (45.0, false));
        assert!((loadout.weapon(0).heat.unwrap().idle_ms - 125.0).abs() < f32::EPSILON);
    }

    #[test]
    fn heat_and_cell_holstered_instances_cool_and_regenerate_while_another_is_active() {
        let loadout = Loadout::new(vec![
            weapon("auto", serde_json::Value::Null),
            weapon("auto", heat(40.0, 40.0, 0.0)),
            weapon("auto", cell(40.0, 4.0, 8.0, 0.0)),
        ]);
        loadout.edit(1, |weapon| {
            let heat = weapon.heat.as_mut().unwrap();
            heat.heat = 100.0;
            heat.overheated = true;
            weapon.cooldown_remaining_ms = 60.0;
        });
        loadout.edit(2, |weapon| {
            weapon.cell.as_mut().unwrap().charge = 0.0;
            weapon.cooldown_remaining_ms = 60.0;
        });

        for _ in 0..4 {
            let _ = loadout.tick(true, None);
        }

        assert_eq!(loadout.active_slot(), 0);
        assert_eq!(live_heat(&loadout, 1), (80.0, true));
        assert!((loadout.weapon(2).cell.unwrap().charge - 4.0).abs() < f32::EPSILON);
        // Only the resource advances on a holstered instance; its cooldown stays frozen.
        assert!((loadout.weapon(1).cooldown_remaining_ms - 60.0).abs() < f32::EPSILON);
        assert!((loadout.weapon(2).cooldown_remaining_ms - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn heat_latch_survives_switching_away_and_back() {
        let loadout = Loadout::new(vec![
            weapon("auto", heat(40.0, 8.0, 0.0)),
            weapon("auto", serde_json::Value::Null),
        ]);
        loadout.edit(0, |weapon| {
            let heat = weapon.heat.as_mut().unwrap();
            heat.heat = 100.0;
            heat.overheated = true;
        });

        let _ = loadout.tick(false, Some(1));
        let _ = loadout.tick(false, None);
        assert_eq!(loadout.active_slot(), 1);
        assert_eq!(loadout.weapon(0).state, WieldableState::Idle);
        let _ = loadout.tick(false, Some(0));
        let _ = loadout.tick(false, None);
        assert_eq!(loadout.active_slot(), 0);

        let (heat, overheated) = live_heat(&loadout, 0);
        assert!(
            overheated,
            "lowering and raising must not clear an overheat"
        );
        assert!(
            heat > 0.0 && heat < 100.0,
            "it kept cooling throughout: {heat}"
        );
        assert!(addresses(&loadout.tick(true, None)).is_empty());
    }

    #[test]
    fn cell_below_cost_dry_fires_and_resets_the_cooldown() {
        let loadout = Loadout::new(vec![weapon("auto", cell(8.0, 4.0, 0.0, 0.0))]);
        assert_eq!(addresses(&loadout.tick(true, None)), vec!["activate"]);
        assert_eq!(addresses(&loadout.tick(true, None)), vec!["activate"]);
        assert!(loadout.weapon(0).cell.unwrap().charge.abs() < f32::EPSILON);

        assert_eq!(addresses(&loadout.tick(true, None)), vec!["dry_fire"]);
        let component = loadout.weapon(0);
        assert!((component.cooldown_remaining_ms - component.cooldown_ms).abs() < f32::EPSILON);
        assert!(component.cell.unwrap().charge.abs() < f32::EPSILON);
    }

    #[test]
    fn cell_regen_waits_for_the_delay_then_refills_to_capacity() {
        // 40/s at 125 ms is 5 charge per tick; the delay is four ticks.
        let loadout = Loadout::new(vec![weapon("semi", cell(40.0, 10.0, 40.0, 500.0))]);
        let _ = loadout.tick(true, None);
        let charge = |loadout: &Loadout| loadout.weapon(0).cell.unwrap().charge;
        assert!((charge(&loadout) - 30.0).abs() < f32::EPSILON);

        for _ in 0..3 {
            let _ = loadout.tick(false, None);
            assert!((charge(&loadout) - 30.0).abs() < f32::EPSILON);
        }
        let _ = loadout.tick(false, None);
        assert!((charge(&loadout) - 35.0).abs() < f32::EPSILON);
        for _ in 0..3 {
            let _ = loadout.tick(false, None);
        }
        assert!((charge(&loadout) - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn heat_remote_crossing_shot_emits_overheat_through_the_host_route() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon_id) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon_id = registry.spawn(Transform::default());
            let mut component = weapon("auto", heat(40.0, 40.0, 10_000.0));
            component.heat.as_mut().unwrap().heat = 80.0;
            registry.set_component(weapon_id, component).unwrap();
            (pawn, weapon_id)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon_id), 1, 1, true, false)],
        );
        assert_eq!(addresses(&events), vec!["activate", "overheat"]);

        let events = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon_id), 1, 2, true, false)],
        );
        assert!(
            addresses(&events).is_empty(),
            "lockout is silent on the host too"
        );
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .heat
                .unwrap()
                .overheated
        );
    }

    #[test]
    fn heat_and_cell_never_advance_on_the_connected_client_wieldable_pass() {
        let loadout = Loadout::new(vec![
            weapon("auto", heat(40.0, 40.0, 0.0)),
            weapon("auto", cell(40.0, 4.0, 8.0, 0.0)),
        ]);
        loadout.edit(0, |weapon| weapon.heat.as_mut().unwrap().heat = 50.0);
        loadout.edit(1, |weapon| weapon.cell.as_mut().unwrap().charge = 10.0);
        let before = (loadout.weapon(0).heat, loadout.weapon(1).cell);

        for select_slot in [None, Some(1), None] {
            let _ = simulate_client_wieldable_tick(
                loadout.registry.clone(),
                &CollisionWorld::default(),
                &HitZoneStore::new(),
                Some(loadout.pawn),
                false,
                select_slot,
                FireButtonState {
                    pressed: true,
                    active: true,
                },
                false,
                0.0,
                DT,
            );
        }

        assert_eq!((loadout.weapon(0).heat, loadout.weapon(1).cell), before);
    }
}
