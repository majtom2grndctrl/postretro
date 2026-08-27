// Weapon command orchestration, impact damage, and reload delivery.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

mod commands;
mod fire;
mod impact;
mod machine;
mod state;

pub(super) use commands::{
    LocalWeaponCommandResult, normalize_all_inventory_liveness, normalize_inventory_liveness,
    refuse_local_switch, run_local_weapon_command, run_local_weapon_command_with_content,
    run_remote_weapon_commands, weapon_fire_command,
};
pub(crate) use commands::{projectile_model_body_rotation, spawn_projectile};
pub(crate) use impact::apply_authorized_weapon_impact_damage;
pub(crate) use state::transition_to_idle;

#[cfg(test)]
pub(super) fn deliver_reload_to_weapon(
    registry: &mut postretro_entities::EntityRegistry,
    pawn: postretro_entities::EntityId,
    weapon: postretro_entities::EntityId,
    reload_pressed: bool,
    tick_dt: f32,
) -> Vec<super::ReloadDelivery> {
    machine::deliver_reload_to_weapon(registry, pawn, weapon, reload_pressed, tick_dt)
}

#[cfg(test)]
mod tests {
    use super::impact::apply_weapon_impact_damage;
    use super::machine::{WeaponMachineTick, tick_weapon_machine};
    use super::state::{WieldableStateEvent, transition_wieldable_state};
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::collision::CollisionWorld;
    use crate::kinematic_mover::MoverTickStateTable;
    use crate::netcode::{
        MovementOwners, NetworkIdAllocator, OpenAuthorizedShots, ingest_hit_declaration_for_test,
    };
    use crate::scripting_systems::hit_zones::HitZoneStore;
    use crate::sim::tests::{
        remote_command, run_local_only_tick, run_remote_only_tick, sim_command, spawn_reload_pair,
        trigger_movement, weapon_component,
    };
    use crate::sim::{
        PostMovementCommand, PredictedProjectileResolution, ReloadDelivery, ReloadOutcome, ShotId,
        advance_predicted, simulate_tick,
    };
    use crate::weapon::tests::{
        ammo_weapon_component as gate_ammo_weapon_component, wall_world,
        weapon_component as gate_weapon_component,
    };
    use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
    use glam::Vec3;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::components::weapon::{
        ReloadFeedback, ReloadFeedbackConsumer, WeaponComponent,
    };
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::data_descriptors::{
        AmmoResource, ReloadStyle, ResolutionMode, WeaponDescriptor, WeaponResource,
    };
    use postretro_entities::{AmmoReserve, EntityId, EntityRegistry, Transform};
    use postretro_foundation::{
        FireMode, ProjectileBodyVisual, ProjectileDescriptor, ProjectileVisual,
    };
    use postretro_net::wire::{self, ClientMessage, HitDeclaration, HitRecord, NetworkId};
    use postretro_scripting_core::reaction_dispatch::ProgressTracker;

    fn fire_command(pressed: bool, active: bool) -> WeaponFireCommand {
        WeaponFireCommand {
            button: FireButtonState { pressed, active },
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        }
    }

    fn ignore_impact(_: &mut EntityRegistry) {}

    fn tick_machine(
        registry: &mut EntityRegistry,
        pawn: Option<EntityId>,
        weapon: EntityId,
        reload: bool,
        command: &WeaponFireCommand,
        tick_dt: f32,
    ) -> WeaponMachineTick {
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon component exists")
            .clone();
        let result = tick_weapon_machine(
            registry,
            pawn,
            weapon,
            &mut component,
            reload,
            command,
            false,
            tick_dt,
        );
        registry.set_component(weapon, component).unwrap();
        result
    }

    fn spawn_gate_weapon(
        registry: &mut EntityRegistry,
        component: WeaponComponent,
    ) -> (EntityId, EntityId) {
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry.set_component(weapon, component).unwrap();
        (pawn, weapon)
    }

    fn spawn_local_pellet_weapon(
        registry: &mut EntityRegistry,
        credit_source: &str,
    ) -> (EntityId, EntityId) {
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let mut component = weapon_component(credit_source);
        component.pellet_count = 8;
        component.spread_degrees = 0.0;
        registry.set_component(weapon, component).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        (pawn, weapon)
    }

    fn spawn_pellet_target(registry: &mut EntityRegistry) -> EntityId {
        let target = registry.spawn(Transform {
            position: Vec3::new(0.0, 0.0, -5.0),
            ..Transform::default()
        });
        registry
            .set_component(
                target,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents: Vec3::splat(0.5),
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        target
    }

    fn projectile_weapon_component(credit_source: &str) -> WeaponComponent {
        let mut component = weapon_component(credit_source);
        component.resolution = ResolutionMode::Projectile;
        component.projectile = Some(ProjectileDescriptor {
            speed: 1.0,
            radius: 0.0,
            lifetime_ms: 5_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/projectiles/test-bolt.png".to_string(),
                    size: 0.25,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0, 1.0, 1.0],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        });
        component
    }

    fn set_reload_style(
        registry: &mut EntityRegistry,
        weapon: EntityId,
        reload_style: ReloadStyle,
    ) {
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon component exists")
            .clone();
        component
            .ammo
            .as_mut()
            .expect("reload fixture has ammo tuning")
            .reload_style = reload_style;
        registry.set_component(weapon, component).unwrap();
    }

    fn refreshed_ammo_descriptor(reload_style: ReloadStyle) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: Some("weapon.test.reload".to_string()),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "bullets.light".to_string(),
                magazine: 10,
                cost_per_shot: 1,
                reserve: 8,
                reload_ms: 100,
                reload_style,
            })),
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        }
    }

    fn spawn_switch_pair(
        registry: &mut EntityRegistry,
        outgoing_lower_ms: u32,
        outgoing_raise_ms: u32,
        incoming_lower_ms: u32,
        incoming_raise_ms: u32,
    ) -> (EntityId, EntityId, EntityId) {
        let pawn = registry.spawn(Transform::default());
        let outgoing = registry.spawn(Transform::default());
        let incoming = registry.spawn(Transform::default());
        let mut outgoing_component = gate_weapon_component(FireMode::Semi, 100.0);
        outgoing_component.lower_ms = outgoing_lower_ms;
        outgoing_component.raise_ms = outgoing_raise_ms;
        let mut incoming_component = gate_weapon_component(FireMode::Semi, 100.0);
        incoming_component.lower_ms = incoming_lower_ms;
        incoming_component.raise_ms = incoming_raise_ms;
        registry
            .set_component(outgoing, outgoing_component)
            .unwrap();
        registry
            .set_component(incoming, incoming_component)
            .unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(outgoing);
        inventory.wieldables[1] = Some(incoming);
        registry.set_component(pawn, inventory).unwrap();
        (pawn, outgoing, incoming)
    }

    #[test]
    fn begin_lower_clears_reload_feedback_for_every_state_row() {
        for (state, existing_remaining_ms) in [
            (WieldableState::Idle, 0),
            (WieldableState::Reloading, 9),
            (WieldableState::ShellLoading, 9),
            (WieldableState::Lowering, 9),
            (WieldableState::Raising, 9),
        ] {
            let mut weapon = gate_weapon_component(FireMode::Semi, 100.0);
            weapon.state = state;
            weapon.state_remaining_ms = existing_remaining_ms;
            weapon.state_total_ms = existing_remaining_ms;
            let feedback_tick = weapon.begin_reload_feedback_tick();
            weapon.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);

            let _ = transition_wieldable_state(
                &mut weapon,
                WieldableStateEvent::BeginLower { duration_ms: 17 },
                None,
            );

            assert_eq!(weapon.state, WieldableState::Lowering);
            assert_eq!(
                weapon.state_remaining_ms,
                if state == WieldableState::Lowering {
                    existing_remaining_ms
                } else {
                    17
                },
                "lowering must not restart when retargeted"
            );
            for consumer in [
                ReloadFeedbackConsumer::Hud,
                ReloadFeedbackConsumer::OwnerProjection,
            ] {
                assert!(
                    weapon.reload_feedback_sample(consumer).endpoint.is_none(),
                    "{state:?} BeginLower must drain reload feedback"
                );
            }
        }
    }

    #[test]
    fn o4_commit_on_a_repoint_tick_applies_to_the_newly_active_instance_once() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, first_target, second_target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let outgoing = registry.spawn(Transform::default());
            let first_target = registry.spawn(Transform::default());
            let second_target = registry.spawn(Transform::default());
            let mut outgoing_component = gate_weapon_component(FireMode::Semi, 100.0);
            outgoing_component.lower_ms = 20;
            let feedback_tick = outgoing_component.begin_reload_feedback_tick();
            outgoing_component.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
            let mut first_target_component = gate_weapon_component(FireMode::Semi, 100.0);
            first_target_component.raise_ms = 15;
            let mut second_target_component = gate_weapon_component(FireMode::Semi, 100.0);
            second_target_component.raise_ms = 15;
            registry
                .set_component(outgoing, outgoing_component)
                .unwrap();
            registry
                .set_component(first_target, first_target_component)
                .unwrap();
            registry
                .set_component(second_target, second_target_component)
                .unwrap();
            let mut inventory = Inventory::default();
            inventory.wieldables[0] = Some(outgoing);
            inventory.wieldables[1] = Some(first_target);
            inventory.wieldables[2] = Some(second_target);
            registry.set_component(pawn, inventory).unwrap();
            (pawn, outgoing, first_target, second_target)
        };
        let collision_world = CollisionWorld::default();
        let hit_zone_store = HitZoneStore::new();
        let command = fire_command(true, true);
        let mut no_impact = ignore_impact;

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            Some(1),
            &command,
            true,
            &collision_world,
            &hit_zone_store,
            0.0,
            0.01,
            &mut no_impact,
        );
        assert!(result.reload_deliveries.is_empty());
        assert!(result.weapon_events.is_empty());
        {
            let registry = registry.borrow();
            let inventory = registry.get_component::<Inventory>(pawn).unwrap();
            let outgoing = registry.get_component::<WeaponComponent>(outgoing).unwrap();
            assert_eq!(inventory.active_slot, 0);
            assert_eq!(inventory.switch_target, Some(1));
            assert_eq!(inventory.switch_origin, Some(0));
            assert_eq!(outgoing.state, WieldableState::Lowering);
            assert_eq!(outgoing.state_remaining_ms, 11);
            assert!(!outgoing.reload_status().1);
            assert!(!outgoing.owner_reload_status().1);
        }

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            Some(2),
            &command,
            true,
            &collision_world,
            &hit_zone_store,
            0.0,
            0.0,
            &mut no_impact,
        );
        assert!(result.reload_deliveries.is_empty());
        assert!(result.weapon_events.is_empty());
        {
            let registry = registry.borrow();
            let inventory = registry.get_component::<Inventory>(pawn).unwrap();
            let outgoing = registry.get_component::<WeaponComponent>(outgoing).unwrap();
            assert_eq!(inventory.switch_target, Some(2));
            assert_eq!(inventory.switch_origin, Some(0));
            assert_eq!(outgoing.state_remaining_ms, 11);
        }

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &command,
            true,
            &collision_world,
            &hit_zone_store,
            0.0,
            0.01,
            &mut no_impact,
        );
        assert!(result.reload_deliveries.is_empty());
        assert!(result.weapon_events.is_empty());
        let registry_ref = registry.borrow();
        let inventory = registry_ref.get_component::<Inventory>(pawn).unwrap();
        let outgoing = registry_ref
            .get_component::<WeaponComponent>(outgoing)
            .unwrap();
        let first_target = registry_ref
            .get_component::<WeaponComponent>(first_target)
            .unwrap();
        let second_target = registry_ref
            .get_component::<WeaponComponent>(second_target)
            .unwrap();
        assert_eq!(inventory.active_slot, 2);
        assert_eq!(inventory.switch_target, None);
        assert_eq!(outgoing.state, WieldableState::Idle);
        assert_eq!(first_target.state, WieldableState::Idle);
        assert_eq!(second_target.state, WieldableState::Raising);
        assert_eq!(second_target.state_remaining_ms, 15);
        assert!(second_target.reload_press_consumed);
    }

    #[test]
    fn o1_lowering_timer_advances_and_expires_through_the_timed_state_predicate() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Semi, 100.0));
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon exists")
            .clone();
        component.state = WieldableState::Lowering;
        component.state_total_ms = 10;
        component.state_remaining_ms = 10;
        registry.set_component(weapon, component).unwrap();

        let first = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(false, false),
            0.004,
        );
        assert!(!first.lowered);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state_remaining_ms,
            6
        );

        let second = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(false, false),
            0.006,
        );
        assert!(second.lowered, "a lowering timer expires at zero");
    }

    #[test]
    fn o2_reload_expiry_completes_before_same_tick_commit_starts_lowering() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, incoming) = {
            let mut registry = registry.borrow_mut();
            let (pawn, outgoing) = spawn_reload_pair(&mut registry, 10, 8, 10, 0);
            let incoming = registry.spawn(Transform::default());
            registry
                .set_component(incoming, gate_weapon_component(FireMode::Semi, 100.0))
                .unwrap();
            let mut outgoing_component = registry
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .clone();
            outgoing_component.lower_ms = 7;
            outgoing_component.state = WieldableState::Reloading;
            outgoing_component.state_total_ms = 10;
            outgoing_component.state_remaining_ms = 10;
            registry
                .set_component(outgoing, outgoing_component)
                .unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(incoming);
            registry.set_component(pawn, inventory).unwrap();
            (pawn, outgoing, incoming)
        };
        let mut no_impact = ignore_impact;

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            Some(1),
            &fire_command(true, true),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.01,
            &mut no_impact,
        );

        assert_eq!(
            result.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon: outgoing,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }],
            "the completed reload is the one lifecycle outcome before lowering"
        );
        assert!(
            result.weapon_events.is_empty(),
            "the commit tick cannot authorize a shot"
        );
        assert_eq!(
            result.repointed_pawn, None,
            "the non-zero lower has only just started"
        );
        let registry = registry.borrow();
        let outgoing = registry.get_component::<WeaponComponent>(outgoing).unwrap();
        assert_eq!(outgoing.state, WieldableState::Lowering);
        assert_eq!(outgoing.state_remaining_ms, 7);
        assert_eq!(outgoing.reload_status().1, false);
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .switch_target,
            Some(1)
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .state,
            WieldableState::Idle
        );
    }

    #[test]
    fn o2_expired_reload_with_zero_lower_repoints_once_without_authorizing_fire() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, incoming) = {
            let mut registry = registry.borrow_mut();
            let (pawn, outgoing) = spawn_reload_pair(&mut registry, 10, 8, 10, 0);
            let incoming = registry.spawn(Transform::default());
            registry
                .set_component(incoming, gate_weapon_component(FireMode::Semi, 100.0))
                .unwrap();
            let mut outgoing_component = registry
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .clone();
            outgoing_component.lower_ms = 0;
            outgoing_component.state = WieldableState::Reloading;
            outgoing_component.state_total_ms = 10;
            outgoing_component.state_remaining_ms = 10;
            registry
                .set_component(outgoing, outgoing_component)
                .unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(incoming);
            registry.set_component(pawn, inventory).unwrap();
            (pawn, outgoing, incoming)
        };
        let mut no_impact = ignore_impact;

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            Some(1),
            &fire_command(true, true),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.01,
            &mut no_impact,
        );

        assert_eq!(
            result.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon: outgoing,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }]
        );
        assert!(result.weapon_events.is_empty());
        assert_eq!(result.repointed_pawn, Some(pawn));
        let registry = registry.borrow();
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .active_slot,
            1
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .state,
            WieldableState::Idle
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .state,
            WieldableState::Raising
        );
    }

    #[test]
    fn o3_commit_during_reload_forfeits_atomic_but_preserves_credited_shells() {
        let atomic_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (atomic_pawn, atomic_weapon) = {
            let mut registry = atomic_registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 0);
            let target = registry.spawn(Transform::default());
            registry
                .set_component(target, gate_weapon_component(FireMode::Semi, 100.0))
                .unwrap();
            let mut weapon_component = registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .clone();
            weapon_component.lower_ms = 10;
            weapon_component.state = WieldableState::Reloading;
            weapon_component.state_total_ms = 100;
            weapon_component.state_remaining_ms = 100;
            registry.set_component(weapon, weapon_component).unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(target);
            registry.set_component(pawn, inventory).unwrap();
            (pawn, weapon)
        };
        let mut no_atomic_impact = ignore_impact;
        let _ = run_local_weapon_command(
            &atomic_registry,
            Some(atomic_pawn),
            false,
            Some(1),
            &fire_command(false, false),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut no_atomic_impact,
        );
        assert_eq!(
            atomic_registry
                .borrow()
                .get_component::<WeaponComponent>(atomic_weapon)
                .unwrap()
                .magazine,
            0,
            "atomic reload transfers nothing when the lower preempts it"
        );

        let shell_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (shell_pawn, shell_weapon) = {
            let mut registry = shell_registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 1);
            let target = registry.spawn(Transform::default());
            registry
                .set_component(target, gate_weapon_component(FireMode::Semi, 100.0))
                .unwrap();
            let mut weapon_component = registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .clone();
            weapon_component.lower_ms = 10;
            weapon_component.state = WieldableState::ShellLoading;
            weapon_component.state_total_ms = 100;
            weapon_component.state_remaining_ms = 100;
            weapon_component.reload_credited = 1;
            registry.set_component(weapon, weapon_component).unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(target);
            registry.set_component(pawn, inventory).unwrap();
            (pawn, weapon)
        };
        let mut no_shell_impact = ignore_impact;
        let _ = run_local_weapon_command(
            &shell_registry,
            Some(shell_pawn),
            false,
            Some(1),
            &fire_command(false, false),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut no_shell_impact,
        );
        let shell_registry = shell_registry.borrow();
        assert_eq!(
            shell_registry
                .get_component::<WeaponComponent>(shell_weapon)
                .unwrap()
                .magazine,
            1,
            "the already-credited shell remains in the magazine"
        );
        assert_eq!(
            shell_registry
                .get_component::<AmmoReserve>(shell_pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn o5_zero_duration_lower_repoints_once_and_clears_the_outgoing_timed_fields() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, incoming) = {
            let mut registry = registry.borrow_mut();
            spawn_switch_pair(&mut registry, 0, 0, 0, 14)
        };
        let mut no_impact = ignore_impact;

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            Some(1),
            &fire_command(false, false),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut no_impact,
        );

        assert_eq!(result.repointed_pawn, Some(pawn));
        let registry = registry.borrow();
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .active_slot,
            1
        );
        let outgoing = registry.get_component::<WeaponComponent>(outgoing).unwrap();
        assert_eq!(outgoing.state, WieldableState::Idle);
        assert_eq!(outgoing.state_total_ms, 0);
        assert_eq!(outgoing.state_remaining_ms, 0);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .state,
            WieldableState::Raising
        );
    }

    #[test]
    fn o6_zero_duration_raise_waits_until_the_incoming_instance_is_next_ticked() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, _, incoming) = {
            let mut registry = registry.borrow_mut();
            spawn_switch_pair(&mut registry, 0, 0, 0, 0)
        };
        let mut no_impact = ignore_impact;
        let mut run = |select_slot| {
            run_local_weapon_command(
                &registry,
                Some(pawn),
                false,
                select_slot,
                &fire_command(false, false),
                false,
                &CollisionWorld::default(),
                &HitZoneStore::new(),
                0.0,
                0.0,
                &mut no_impact,
            )
        };

        let _ = run(Some(1));
        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .state,
            WieldableState::Raising,
            "the repoint tick does not tick the incoming instance a second time"
        );
        let _ = run(None);
        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .state,
            WieldableState::Idle
        );
    }

    #[test]
    fn o7_lower_timer_overshoot_does_not_shorten_the_incoming_full_raise() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, _, incoming) = {
            let mut registry = registry.borrow_mut();
            spawn_switch_pair(&mut registry, 10, 0, 0, 30)
        };
        let mut no_impact = ignore_impact;
        let _ = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            Some(1),
            &fire_command(false, false),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.02,
            &mut no_impact,
        );

        let incoming = registry
            .borrow()
            .get_component::<WeaponComponent>(incoming)
            .unwrap()
            .clone();
        assert_eq!(incoming.state, WieldableState::Raising);
        assert_eq!(incoming.state_total_ms, 30);
        assert_eq!(incoming.state_remaining_ms, 30);
    }

    #[test]
    fn o8_reselecting_a_terminal_outgoing_instance_starts_a_clean_full_raise() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, _) = {
            let mut registry = registry.borrow_mut();
            spawn_switch_pair(&mut registry, 0, 23, 0, 0)
        };
        let mut no_impact = ignore_impact;
        let mut run = |select_slot| {
            run_local_weapon_command(
                &registry,
                Some(pawn),
                false,
                select_slot,
                &fire_command(false, false),
                false,
                &CollisionWorld::default(),
                &HitZoneStore::new(),
                0.0,
                0.0,
                &mut no_impact,
            )
        };

        let _ = run(Some(1));
        let _ = run(None);
        let _ = run(Some(0));

        let outgoing = registry
            .borrow()
            .get_component::<WeaponComponent>(outgoing)
            .unwrap()
            .clone();
        assert_eq!(outgoing.state, WieldableState::Raising);
        assert_eq!(outgoing.state_total_ms, 23);
        assert_eq!(outgoing.state_remaining_ms, 23);
    }

    #[test]
    fn o9_fire_held_across_a_switch_allows_auto_but_requires_a_new_semi_press() {
        let mut registry = EntityRegistry::new();
        let (pawn, auto) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Auto, 100.0));
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                auto,
                false,
                &fire_command(false, true),
                0.0,
            )
            .authorization,
            WeaponFireAuthorization::Accepted
        );

        let mut semi = gate_weapon_component(FireMode::Semi, 100.0);
        semi.shoot_press_consumed = true;
        let (_, semi_id) = spawn_gate_weapon(&mut registry, semi);
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                semi_id,
                false,
                &fire_command(true, true),
                0.0,
            )
            .authorization,
            WeaponFireAuthorization::Rejected,
            "an incoming semi instance requires release and a new press"
        );
    }

    #[test]
    fn o10_held_reload_is_consumed_on_repoint_and_does_not_start_on_the_incoming_weapon() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, _, incoming) = {
            let mut registry = registry.borrow_mut();
            spawn_switch_pair(&mut registry, 0, 0, 0, 0)
        };
        let mut no_impact = ignore_impact;
        let mut run = |select_slot| {
            run_local_weapon_command(
                &registry,
                Some(pawn),
                false,
                select_slot,
                &fire_command(false, false),
                true,
                &CollisionWorld::default(),
                &HitZoneStore::new(),
                0.0,
                0.0,
                &mut no_impact,
            )
        };

        let _ = run(Some(1));
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .reload_press_consumed
        );
        let _ = run(None);
        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(incoming)
                .unwrap()
                .state,
            WieldableState::Idle
        );
    }

    #[test]
    fn o11_deploy_clamp_keeps_remaining_cooldown_after_a_short_round_trip_switch() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, _) = {
            let mut registry = registry.borrow_mut();
            let ids = spawn_switch_pair(&mut registry, 10, 20, 0, 0);
            let mut component = registry
                .get_component::<WeaponComponent>(ids.1)
                .unwrap()
                .clone();
            component.cooldown_remaining_ms = 100.0;
            registry.set_component(ids.1, component).unwrap();
            ids
        };
        let mut no_impact = ignore_impact;
        let mut run = |select_slot, tick_dt| {
            run_local_weapon_command(
                &registry,
                Some(pawn),
                false,
                select_slot,
                &fire_command(false, false),
                false,
                &CollisionWorld::default(),
                &HitZoneStore::new(),
                0.0,
                tick_dt,
                &mut no_impact,
            )
        };

        let _ = run(Some(1), 0.0);
        let _ = run(None, 0.01);
        let _ = run(None, 0.0);
        let _ = run(Some(0), 0.0);

        assert!(
            (registry
                .borrow()
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .cooldown_remaining_ms
                - 90.0)
                .abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn o12_inactive_weapon_cooldown_freezes_at_its_last_active_value() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, outgoing, _) = {
            let mut registry = registry.borrow_mut();
            let ids = spawn_switch_pair(&mut registry, 10, 0, 0, 0);
            let mut component = registry
                .get_component::<WeaponComponent>(ids.1)
                .unwrap()
                .clone();
            component.cooldown_remaining_ms = 100.0;
            registry.set_component(ids.1, component).unwrap();
            ids
        };
        let mut no_impact = ignore_impact;
        let mut run = |select_slot, tick_dt| {
            run_local_weapon_command(
                &registry,
                Some(pawn),
                false,
                select_slot,
                &fire_command(false, false),
                false,
                &CollisionWorld::default(),
                &HitZoneStore::new(),
                0.0,
                tick_dt,
                &mut no_impact,
            )
        };

        let _ = run(Some(1), 0.0);
        let _ = run(None, 0.01);
        let _ = run(None, 0.0);
        for _ in 0..10 {
            let _ = run(None, 0.05);
        }

        assert!(
            (registry
                .borrow()
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .cooldown_remaining_ms
                - 90.0)
                .abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn o46_expiry_loop_admits_lowering_and_exits_after_its_repoint_transition() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Semi, 100.0));
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.state = WieldableState::Lowering;
        component.state_total_ms = 0;
        component.state_remaining_ms = 0;
        registry.set_component(weapon, component).unwrap();

        let tick = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(false, false),
            0.0,
        );
        assert!(tick.lowered, "the timed-state expiry loop reaches Lowered");
    }

    #[test]
    fn o25_switch_refusal_restores_origin_after_local_repoint_and_clears_equip_state() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let origin = registry.spawn(Transform::default());
        let refused = registry.spawn(Transform::default());
        registry
            .set_component(origin, gate_weapon_component(FireMode::Semi, 100.0))
            .unwrap();
        let mut refused_component = gate_weapon_component(FireMode::Semi, 100.0);
        refused_component.state = WieldableState::Raising;
        refused_component.state_total_ms = 15;
        refused_component.state_remaining_ms = 11;
        registry.set_component(refused, refused_component).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(origin);
        inventory.wieldables[1] = Some(refused);
        inventory.active_slot = 1;
        inventory.switch_origin = Some(0);
        registry.set_component(pawn, inventory).unwrap();

        assert!(refuse_local_switch(&mut registry, pawn, 1, 0));

        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        let refused = registry.get_component::<WeaponComponent>(refused).unwrap();
        assert_eq!(inventory.active_slot, 0);
        assert_eq!(inventory.switch_target, None);
        assert_eq!(inventory.switch_origin, None);
        assert_eq!(refused.state, WieldableState::Idle);
        assert_eq!(refused.state_total_ms, 0);
        assert_eq!(refused.state_remaining_ms, 0);
    }

    #[test]
    fn o21_active_instance_despawn_during_lower_abandons_switch_without_raise() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, stale, live) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let stale = registry.spawn(Transform::default());
            let live = registry.spawn(Transform::default());
            let mut lowering = gate_weapon_component(FireMode::Semi, 100.0);
            lowering.state = WieldableState::Lowering;
            lowering.state_total_ms = 20;
            lowering.state_remaining_ms = 10;
            registry.set_component(stale, lowering).unwrap();
            registry
                .set_component(live, gate_weapon_component(FireMode::Semi, 100.0))
                .unwrap();
            let mut inventory = Inventory::default();
            inventory.wieldables[0] = Some(stale);
            inventory.wieldables[1] = Some(live);
            inventory.switch_target = Some(1);
            inventory.switch_origin = Some(0);
            registry.set_component(pawn, inventory).unwrap();
            registry.despawn(stale).unwrap();
            (pawn, stale, live)
        };
        let mut no_impact = ignore_impact;

        let _ = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(false, false),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut no_impact,
        );

        let registry = registry.borrow();
        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.wieldables[0], None);
        assert_eq!(inventory.active_slot, 1);
        assert_eq!(inventory.active_wieldable(), Some(live));
        assert_eq!(inventory.switch_target, None);
        assert_eq!(inventory.switch_origin, None);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(live)
                .unwrap()
                .state,
            WieldableState::Idle,
            "liveness repair selects the first occupied slot without starting a raise"
        );
        assert!(!registry.exists(stale));
    }

    // Regression: host-mirrored inventories were normalized only while consuming a
    // weapon command, so a despawned sibling could remain active indefinitely.
    #[test]
    fn commandless_liveness_sweep_repairs_active_and_target_and_surfaces_repoint() {
        let mut registry = EntityRegistry::new();

        let active_stale_pawn = registry.spawn(Transform::default());
        let stale_active = registry.spawn(Transform::default());
        let replacement = registry.spawn(Transform::default());
        registry
            .set_component(stale_active, gate_weapon_component(FireMode::Semi, 100.0))
            .unwrap();
        registry
            .set_component(replacement, gate_weapon_component(FireMode::Semi, 100.0))
            .unwrap();
        let mut active_stale_inventory = Inventory::default();
        active_stale_inventory.wieldables[0] = Some(stale_active);
        active_stale_inventory.wieldables[1] = Some(replacement);
        registry
            .set_component(active_stale_pawn, active_stale_inventory)
            .unwrap();
        registry.despawn(stale_active).unwrap();

        let target_stale_pawn = registry.spawn(Transform::default());
        let retained_active = registry.spawn(Transform::default());
        let stale_target = registry.spawn(Transform::default());
        let mut lowering = gate_weapon_component(FireMode::Semi, 100.0);
        lowering.state = WieldableState::Lowering;
        lowering.state_total_ms = 20;
        lowering.state_remaining_ms = 10;
        registry.set_component(retained_active, lowering).unwrap();
        registry
            .set_component(stale_target, gate_weapon_component(FireMode::Semi, 100.0))
            .unwrap();
        let mut target_stale_inventory = Inventory::default();
        target_stale_inventory.wieldables[0] = Some(retained_active);
        target_stale_inventory.wieldables[1] = Some(stale_target);
        target_stale_inventory.switch_target = Some(1);
        target_stale_inventory.switch_origin = Some(0);
        registry
            .set_component(target_stale_pawn, target_stale_inventory)
            .unwrap();
        registry.despawn(stale_target).unwrap();

        let attachment_dirty = normalize_all_inventory_liveness(&mut registry);

        assert_eq!(attachment_dirty, vec![active_stale_pawn]);
        let repaired = registry
            .get_component::<Inventory>(active_stale_pawn)
            .unwrap();
        assert_eq!(repaired.active_slot, 1);
        assert_eq!(repaired.active_wieldable(), Some(replacement));
        let abandoned = registry
            .get_component::<Inventory>(target_stale_pawn)
            .unwrap();
        assert_eq!(abandoned.active_slot, 0);
        assert_eq!(abandoned.switch_target, None);
        assert_eq!(abandoned.switch_origin, None);
        let retained = registry
            .get_component::<WeaponComponent>(retained_active)
            .unwrap();
        assert_eq!(retained.state, WieldableState::Idle);
        assert_eq!(retained.state_remaining_ms, 0);
    }

    #[test]
    fn despawned_switch_target_abandons_lower_and_keeps_first_live_active() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, active, target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let active = registry.spawn(Transform::default());
            let target = registry.spawn(Transform::default());
            let mut active_component = gate_weapon_component(FireMode::Semi, 100.0);
            active_component.state = WieldableState::Lowering;
            active_component.state_total_ms = 20;
            active_component.state_remaining_ms = 10;
            registry.set_component(active, active_component).unwrap();
            registry
                .set_component(target, gate_weapon_component(FireMode::Semi, 100.0))
                .unwrap();
            let mut inventory = Inventory::default();
            inventory.wieldables[0] = Some(active);
            inventory.wieldables[1] = Some(target);
            inventory.switch_target = Some(1);
            inventory.switch_origin = Some(0);
            registry.set_component(pawn, inventory).unwrap();
            registry.despawn(target).unwrap();
            (pawn, active, target)
        };

        let mut no_impact = ignore_impact;
        let _ = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(false, false),
            false,
            &CollisionWorld::default(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut no_impact,
        );

        let registry = registry.borrow();
        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.wieldables[1], None);
        assert_eq!(inventory.active_slot, 0);
        assert_eq!(inventory.switch_target, None);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(active)
                .unwrap()
                .state,
            WieldableState::Idle
        );
        assert!(!registry.exists(target));
    }

    #[test]
    fn reload_switch_gate_resolves_weapon_override_before_mod_global() {
        let attempt_switch = |mod_block_during_reload: bool, weapon_override: Option<bool>| {
            let registry = Rc::new(RefCell::new(EntityRegistry::new()));
            let (pawn, outgoing) = {
                let mut registry_ref = registry.borrow_mut();
                let pawn = registry_ref.spawn(Transform::default());
                let outgoing = registry_ref.spawn(Transform::default());
                let target = registry_ref.spawn(Transform::default());
                let mut outgoing_component = gate_weapon_component(FireMode::Semi, 100.0);
                outgoing_component.lower_ms = 10;
                outgoing_component.block_during_reload = weapon_override;
                outgoing_component.state = WieldableState::Reloading;
                outgoing_component.state_remaining_ms = 20;
                outgoing_component.state_total_ms = 20;
                registry_ref
                    .set_component(outgoing, outgoing_component)
                    .unwrap();
                registry_ref
                    .set_component(target, gate_weapon_component(FireMode::Semi, 100.0))
                    .unwrap();
                let mut inventory = Inventory::default();
                inventory.wieldables[0] = Some(outgoing);
                inventory.wieldables[1] = Some(target);
                registry_ref.set_component(pawn, inventory).unwrap();
                (pawn, outgoing)
            };
            let collision_world = CollisionWorld::default();
            let hit_zone_store = HitZoneStore::new();
            let mut no_impact = ignore_impact;
            let _ = run_local_weapon_command(
                &registry,
                Some(pawn),
                mod_block_during_reload,
                Some(1),
                &fire_command(false, false),
                false,
                &collision_world,
                &hit_zone_store,
                0.0,
                0.0,
                &mut no_impact,
            );
            let registry_ref = registry.borrow();
            (
                registry_ref
                    .get_component::<WeaponComponent>(outgoing)
                    .unwrap()
                    .state,
                registry_ref
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .switch_target,
            )
        };

        assert_eq!(
            attempt_switch(true, None),
            (WieldableState::Reloading, None)
        );
        assert_eq!(
            attempt_switch(true, Some(false)),
            (WieldableState::Lowering, Some(1)),
            "a weapon override can allow reload interruption under a blocking mod policy"
        );
        assert_eq!(
            attempt_switch(false, Some(true)),
            (WieldableState::Reloading, None),
            "a weapon override can block reload interruption under a permissive mod policy"
        );
    }

    #[test]
    fn semi_weapon_fires_once_per_press() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Semi, 100.0));

        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(true, true),
                0.0
            )
            .authorization,
            WeaponFireAuthorization::Accepted
        );
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(true, true),
                0.2
            )
            .authorization,
            WeaponFireAuthorization::Rejected
        );
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(false, false),
                0.0
            )
            .authorization,
            WeaponFireAuthorization::Rejected
        );
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(true, true),
                0.0
            )
            .authorization,
            WeaponFireAuthorization::Accepted
        );
    }

    #[test]
    fn auto_weapon_fires_repeatedly_when_held_after_cooldown() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Auto, 30.0));

        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(true, true),
                0.0
            )
            .authorization,
            WeaponFireAuthorization::Accepted
        );
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(false, true),
                0.016
            )
            .authorization,
            WeaponFireAuthorization::Rejected
        );
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(false, true),
                0.016
            )
            .authorization,
            WeaponFireAuthorization::Accepted
        );
    }

    #[test]
    fn below_cost_is_empty_at_state_seam_and_emits_only_dry_fire() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon_id) = spawn_gate_weapon(
            &mut registry,
            gate_ammo_weapon_component(FireMode::Semi, 100.0, 2, 3),
        );
        let command = fire_command(true, true);
        let result = tick_machine(&mut registry, Some(pawn), weapon_id, false, &command, 0.0);
        assert_eq!(result.authorization, WeaponFireAuthorization::Empty);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let events = weapon::tick_resolved_component(
            &registry,
            &mut component,
            "weapon.unknown",
            0,
            &command,
            &postretro_foundation::WeaponPlacementDescriptor::default(),
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            result.authorization,
        );
        assert_eq!(events.event_names(), vec!["dry_fire"]);
        assert_eq!(component.magazine, 2);
    }

    // Regression: a held Auto trigger emitted dry_fire on every fixed tick.
    #[test]
    fn empty_auto_weapon_emits_once_per_fire_interval() {
        let mut registry = EntityRegistry::new();
        let mut component = gate_ammo_weapon_component(FireMode::Auto, 100.0, 1, 1);
        component.magazine = 0;
        let (pawn, weapon) = spawn_gate_weapon(&mut registry, component);

        let first = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(first.authorization, WeaponFireAuthorization::Empty);
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(false, true),
                0.04
            )
            .authorization,
            WeaponFireAuthorization::Rejected
        );
        assert_eq!(
            tick_machine(
                &mut registry,
                Some(pawn),
                weapon,
                false,
                &fire_command(false, true),
                0.061
            )
            .authorization,
            WeaponFireAuthorization::Empty
        );
    }

    #[test]
    fn reload_in_flight_silently_blocks_without_cancelling_or_spending() {
        let mut registry = EntityRegistry::new();
        let mut component = gate_ammo_weapon_component(FireMode::Semi, 100.0, 12, 2);
        component.state = WieldableState::Reloading;
        component.state_remaining_ms = 450;
        component.state_total_ms = 900;
        let (pawn, weapon) = spawn_gate_weapon(&mut registry, component);

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Rejected);
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.magazine, 12);
        assert_eq!(component.state_remaining_ms, 450);
        assert!((component.cooldown_remaining_ms - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pawnless_ammo_weapon_fires_and_cools_but_preserves_the_reload_edge() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);

        let fired = tick_machine(
            &mut registry,
            None,
            weapon,
            true,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(fired.authorization, WeaponFireAuthorization::Accepted);
        assert!(fired.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.magazine, 1);
        assert!((component.cooldown_remaining_ms - 100.0).abs() < f32::EPSILON);
        assert!(!component.reload_press_consumed);

        let cooling = tick_machine(
            &mut registry,
            None,
            weapon,
            true,
            &fire_command(false, false),
            0.05,
        );
        assert_eq!(cooling.authorization, WeaponFireAuthorization::Rejected);
        assert!(cooling.deliveries.is_empty());
        let cooldown = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .cooldown_remaining_ms;
        assert!((cooldown - 50.0).abs() < f32::EPSILON);

        let restored_pawn = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(false, false),
            0.0,
        );
        assert_eq!(
            restored_pawn.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
    }

    #[test]
    fn despawned_pawn_id_is_pawnless_for_reload_entry_and_expiry() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        registry.despawn(pawn).unwrap();

        // Regression: Option presence treated a stale pawn id as live ownership,
        // emitting blocked-empty on entry.
        let entry = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(false, false),
            0.0,
        );
        assert!(entry.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert!(!component.reload_press_consumed);

        let mut component = component.clone();
        component.state = WieldableState::ShellLoading;
        component.state_remaining_ms = 1;
        component.state_total_ms = 1;
        component.reload_credited = 3;
        registry.set_component(weapon, component).unwrap();

        // Regression: stale ownership also emitted a completed outcome on expiry.
        let expiry = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(false, false),
            0.001,
        );
        assert!(expiry.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.reload_credited, 0);
    }

    #[test]
    fn resourceless_weapon_fires_without_magazine_gating_or_consumption() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Semi, 100.0));

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Accepted);
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert!(component.ammo.is_none());
        assert_eq!(component.magazine, 0);
    }

    #[test]
    fn ammo_shot_consumes_effective_cost_once_and_resolves_normally() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon_id) = spawn_gate_weapon(
            &mut registry,
            gate_ammo_weapon_component(FireMode::Semi, 100.0, 12, 2),
        );
        let command = fire_command(true, true);
        let result = tick_machine(&mut registry, Some(pawn), weapon_id, false, &command, 0.0);
        assert_eq!(result.authorization, WeaponFireAuthorization::Accepted);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let events = weapon::tick_resolved_component(
            &registry,
            &mut component,
            "weapon.unknown",
            0,
            &command,
            &postretro_foundation::WeaponPlacementDescriptor::default(),
            &wall_world(),
            &HitZoneStore::new(),
            0.0,
            result.authorization,
        );
        assert!(!events.impacts.is_empty());
        assert_eq!(component.magazine, 10);
    }

    #[test]
    fn ammo_shot_spends_cost_on_open_space_miss() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon_id) = spawn_gate_weapon(
            &mut registry,
            gate_ammo_weapon_component(FireMode::Semi, 100.0, 12, 2),
        );
        let command = fire_command(true, true);
        let result = tick_machine(&mut registry, Some(pawn), weapon_id, false, &command, 0.0);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let events = weapon::tick_resolved_component(
            &registry,
            &mut component,
            "weapon.unknown",
            0,
            &command,
            &postretro_foundation::WeaponPlacementDescriptor::default(),
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            result.authorization,
        );
        assert!(events.impacts.is_empty());
        assert_eq!(component.magazine, 10);
    }

    #[test]
    fn open_space_shot_consumes_cooldown_without_impact() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon_id) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Semi, 100.0));
        let command = fire_command(true, true);
        let result = tick_machine(&mut registry, Some(pawn), weapon_id, false, &command, 0.0);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let events = weapon::tick_resolved_component(
            &registry,
            &mut component,
            "weapon.unknown",
            0,
            &command,
            &postretro_foundation::WeaponPlacementDescriptor::default(),
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            result.authorization,
        );
        assert!(events.impacts.is_empty());
        assert!((component.cooldown_remaining_ms - 100.0).abs() < 1.0e-5);
    }

    #[test]
    fn state_only_fire_advances_cooldown_without_hitscan_events() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) =
            spawn_gate_weapon(&mut registry, gate_weapon_component(FireMode::Semi, 100.0));
        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Accepted);
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert!((component.cooldown_remaining_ms - 100.0).abs() < 1.0e-5);
    }

    #[test]
    fn remote_fire_authorizes_shot_and_does_not_damage_by_raycast() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon, target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            let mut component = weapon_component("weapon.test.remote");
            component.pellet_count = 8;
            registry.set_component(weapon, component).unwrap();
            let target = registry.spawn(Transform::default());
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: None,
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            (pawn, weapon, target)
        };

        let shot_id = ShotId::from_parts(NetworkId(42), 9);
        let events = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 9, true, false)],
        );

        assert_eq!(events.authorized_shots.len(), 1);
        assert_eq!(events.authorized_shots[0].shot.shot_id, shot_id);
        assert_eq!(events.authorized_shots[0].shot.pawn, pawn);
        assert_eq!(events.authorized_shots[0].shot.fire_tick, 33);
        assert_eq!(events.authorized_shots[0].shot.pellet_count, 8);
        assert_eq!(events.authorized_shots[0].owner_client_id, 7);
        assert!(
            events.rejected_remote_projectile_fires.is_empty(),
            "accepted hitscan/pellet FIRE keeps its existing declaration-time verdict path"
        );
        assert_eq!(events.weapon, vec!["activate"]);
        let registry = registry.borrow();
        let weapon_state = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert!((weapon_state.cooldown_remaining_ms - 100.0).abs() < f32::EPSILON);
        assert_eq!(
            weapon_state.shells_fired, 0,
            "the host mints remote authorization but never samples the client's pellet fan"
        );
        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert!((health.current - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn local_projectile_fire_spawns_a_deferred_visual_projectile_without_hitscan_damage() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon, target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(
                    weapon,
                    projectile_weapon_component("weapon.test.projectile"),
                )
                .expect("projectile weapon attaches");
            let mut inventory = Inventory::default();
            inventory.wieldables[0] = Some(weapon);
            registry
                .set_component(pawn, inventory)
                .expect("pawn wields projectile weapon");
            let target = registry.spawn(Transform {
                position: Vec3::new(0.0, 0.0, -0.75),
                ..Transform::default()
            });
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: Some(Hitbox {
                            half_extents: Vec3::splat(0.1),
                            offset: Vec3::ZERO,
                        }),
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .expect("target health attaches");
            (pawn, weapon, target)
        };
        let mut ignore_impact = ignore_impact;

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(true, true),
            false,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            1.0 / 60.0,
            &mut ignore_impact,
        );

        assert_eq!(result.weapon_events, vec!["activate", "spawned"]);
        let [projectile] = result.projectile_spawns.as_slice() else {
            panic!("accepted projectile fire must produce exactly one projectile");
        };
        let registry = registry.borrow();
        assert!(
            registry
                .get_component::<postretro_entities::components::projectile::ProjectileComponent>(
                    *projectile
                )
                .expect("projectile carries gameplay flight state")
                .spawned,
            "the projectile is marked to skip the fire tick's advance pass"
        );
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::sprite_visual::SpriteVisual>(
                    *projectile
                )
                .expect("descriptor body attaches at fire time")
                .sprite,
            "sprites/projectiles/test-bolt.png"
        );
        assert!(
            (registry
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current
                - 100.0)
                .abs()
                <= f32::EPSILON,
            "fire itself emits no hitscan impact; damage waits for projectile advance"
        );
        assert!(registry.exists(weapon));
    }

    #[test]
    fn rejected_remote_projectile_fire_mints_neither_authorization_nor_visual() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            let mut component = projectile_weapon_component("weapon.test.projectile");
            component.cooldown_remaining_ms = 100.0;
            registry
                .set_component(weapon, component)
                .expect("projectile weapon attaches");
            (pawn, weapon)
        };

        let events = run_remote_only_tick(
            registry,
            &[remote_command(pawn, Some(weapon), 42, 9, true, false)],
        );

        assert!(events.authorized_shots.is_empty());
        assert!(events.remote_projectile_presentation_launches.is_empty());
        assert_eq!(
            events.rejected_remote_projectile_fires,
            vec![crate::sim::RemoteProjectileFireRejection {
                owner_client_id: 7,
                shot_id: ShotId::from_parts(NetworkId(42), 9),
            }],
            "the host emits an immediate owner-private correction instead of waiting for flight expiry"
        );
        assert!(events.weapon.is_empty());
    }

    #[test]
    fn rejected_remote_projectile_fire_cannot_later_declare_plausible_damage() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon, target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry
                .set_component(pawn, trigger_movement())
                .expect("remote pawn has a live fire origin");
            let weapon = registry.spawn(Transform::default());
            let mut component = projectile_weapon_component("weapon.test.projectile");
            component.cooldown_remaining_ms = 100.0;
            registry
                .set_component(weapon, component)
                .expect("projectile weapon attaches");
            let target = spawn_pellet_target(&mut registry);
            (pawn, weapon, target)
        };
        let shot_id = ShotId::from_parts(NetworkId(42), 9);
        let events = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 9, true, false)],
        );
        assert!(
            events.authorized_shots.is_empty(),
            "cooldown rejection mints no authority for the declared shot"
        );
        assert_eq!(events.rejected_remote_projectile_fires.len(), 1);
        assert_eq!(events.rejected_remote_projectile_fires[0].shot_id, shot_id);

        let mut allocator = NetworkIdAllocator::new();
        allocator.stamp(pawn);
        let target_network_id = allocator.stamp(target);
        let mut owners = MovementOwners::new();
        owners.set(pawn, 7);
        let mut open_shots = OpenAuthorizedShots::new();
        for open in events.authorized_shots {
            open_shots.record(open.shot, open.owner_client_id);
        }
        let declaration = HitDeclaration {
            shot_id: shot_id.raw(),
            records: vec![HitRecord {
                target: target_network_id.0,
                point: Vec3::new(0.0, 0.5, -5.0).to_array(),
                zone: None,
            }],
        };

        let (fire_accepted, hit_accepted) = ingest_hit_declaration_for_test(
            &mut registry.borrow_mut(),
            &CollisionWorld::new(),
            &allocator,
            &owners,
            &mut open_shots,
            7,
            &declaration,
        );
        assert!(!fire_accepted);
        assert!(!hit_accepted);
        assert_eq!(
            registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            100.0,
            "an unbound declaration cannot apply host damage"
        );
    }

    #[test]
    fn connected_client_projectile_declares_later_and_host_applies_authorized_credit() {
        let host_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (host_pawn, host_weapon, host_target) = {
            let mut registry = host_registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry
                .set_component(pawn, trigger_movement())
                .expect("host remote pawn has a fire-time eye");
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(
                    weapon,
                    projectile_weapon_component("weapon.test.projectile"),
                )
                .expect("host projectile weapon attaches");
            let target = registry.spawn(Transform {
                position: Vec3::new(0.0, 0.0, -2.0),
                ..Transform::default()
            });
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: Some(Hitbox {
                            half_extents: Vec3::splat(0.5),
                            offset: Vec3::ZERO,
                        }),
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .expect("host target has health");
            (pawn, weapon, target)
        };
        let shot_id = ShotId::from_parts(NetworkId(42), 9);
        let host_fire = run_remote_only_tick(
            host_registry.clone(),
            &[remote_command(
                host_pawn,
                Some(host_weapon),
                42,
                9,
                true,
                false,
            )],
        );
        let [authorized] = host_fire.authorized_shots.as_slice() else {
            panic!("accepted remote projectile fire must mint exactly one authorized shot");
        };
        assert_eq!(authorized.shot.shot_id, shot_id);
        assert!(authorized.shot.is_projectile);
        assert!(
            authorized.shot.fire_origin.is_finite()
                && authorized.shot.fire_origin.y > 0.45
                && authorized.shot.fire_origin.y <= 0.5,
            "the accepted remote fire freezes its live eye origin after this tick's movement"
        );

        let client_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (client_pawn, client_weapon, client_target) = {
            let mut registry = client_registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            let target = registry.spawn(Transform {
                position: Vec3::new(0.0, 0.0, -2.0),
                ..Transform::default()
            });
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: Some(Hitbox {
                            half_extents: Vec3::splat(0.5),
                            offset: Vec3::ZERO,
                        }),
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .expect("client target has a predictable hitbox");
            (pawn, weapon, target)
        };
        let mut client_weapon_component = projectile_weapon_component("weapon.test.projectile");
        let launch = weapon::resolve_client_fire(
            &mut client_weapon_component,
            "weapon.test.projectile",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &postretro_foundation::WeaponPlacementDescriptor::default(),
            None,
            9,
            &CollisionWorld::new(),
            &client_registry.borrow(),
            &HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("connected client accepts its local fire intent")
        .projectile_launch
        .expect("projectile resolution creates deferred flight rather than a same-frame hit");
        let client_projectile = spawn_projectile(
            &mut client_registry.borrow_mut(),
            client_pawn,
            client_weapon,
            launch,
            Some(shot_id.raw()),
        )
        .expect("connected client has space for its predicted projectile");
        assert!(client_registry.borrow().exists(client_projectile));

        let mut resolutions = Vec::new();
        advance_predicted(
            &client_registry,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut |resolution| resolutions.push(resolution),
        );
        assert!(
            resolutions.is_empty(),
            "the launch pass clears spawned state without declaring a same-tick impact"
        );
        advance_predicted(
            &client_registry,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            2.0,
            &mut |resolution| resolutions.push(resolution),
        );
        let [
            PredictedProjectileResolution::Impact {
                shot_id: declared_shot_id,
                impact,
            },
        ] = resolutions.as_slice()
        else {
            panic!("later predicted projectile advance must resolve exactly one impact");
        };
        assert_eq!(*declared_shot_id, shot_id.raw());
        assert_eq!(impact.target, Some(client_target));
        assert_eq!(
            client_registry
                .borrow()
                .get_component::<HealthComponent>(client_target)
                .expect("client target remains a presentation query candidate")
                .current,
            100.0,
            "client projectile prediction emits a declaration but never changes client health"
        );

        let mut allocator = NetworkIdAllocator::new();
        allocator.stamp(host_pawn);
        let host_target_network_id = allocator.stamp(host_target);
        let mut owners = MovementOwners::new();
        owners.set(host_pawn, 7);
        let mut open_shots = OpenAuthorizedShots::new();
        open_shots.record(authorized.shot.clone(), authorized.owner_client_id);
        let declaration = HitDeclaration {
            shot_id: *declared_shot_id,
            records: vec![HitRecord {
                target: host_target_network_id.0,
                point: impact.point.to_array(),
                zone: impact.zone.clone(),
            }],
        };
        let bytes = wire::encode(&ClientMessage::HitDeclaration(declaration));
        let ClientMessage::HitDeclaration(delivered) =
            wire::decode(&bytes).expect("client declaration survives the real input-wire encoding")
        else {
            panic!("encoded declaration retains its input message variant");
        };
        let (fire_accepted, hit_accepted) = ingest_hit_declaration_for_test(
            &mut host_registry.borrow_mut(),
            &CollisionWorld::new(),
            &allocator,
            &owners,
            &mut open_shots,
            7,
            &delivered,
        );
        assert!(fire_accepted);
        assert!(hit_accepted);
        let host_registry = host_registry.borrow();
        let host_health = host_registry
            .get_component::<HealthComponent>(host_target)
            .expect("host target remains live after nonlethal impact");
        assert_eq!(host_health.current, 90.0);
        assert_eq!(
            host_health
                .contributor_ledger
                .recorded_damage_by_source("weapon.test.projectile"),
            Some(10.0),
            "the host credits the authorized projectile weapon rather than the client"
        );
        let credit = host_health
            .contributor_ledger
            .entries()
            .first()
            .expect("accepted host impact records attacker credit");
        assert_eq!(credit.last_attacker, Some(host_pawn));
        assert_eq!(credit.last_weapon, Some(host_weapon));
    }

    // Regression: a delayed command for a despawned remote pawn cancelled and
    // fired its stale weapon, minting an OpenAuthorizedShot.
    #[test]
    fn stale_remote_pawn_command_is_silent_and_does_not_mutate_weapon() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon, before) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            let mut component = registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .clone();
            component.state = WieldableState::ShellLoading;
            component.state_remaining_ms = 50;
            component.state_total_ms = 100;
            registry.set_component(weapon, component.clone()).unwrap();
            registry.despawn(pawn).unwrap();
            (pawn, weapon, component)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 9, true, true)],
        );

        assert!(events.authorized_shots.is_empty());
        assert!(events.weapon.is_empty());
        assert!(events.reload_deliveries.is_empty());
        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap(),
            &before
        );
    }

    #[test]
    fn remote_fire_for_two_pawns_updates_only_their_mapped_weapons() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b, idle_weapon) = {
            let mut registry = registry.borrow_mut();
            let pawn_a = registry.spawn(Transform::default());
            let weapon_a = registry.spawn(Transform::default());
            registry
                .set_component(weapon_a, weapon_component("weapon.test.a"))
                .unwrap();
            let pawn_b = registry.spawn(Transform::default());
            let weapon_b = registry.spawn(Transform::default());
            registry
                .set_component(weapon_b, weapon_component("weapon.test.b"))
                .unwrap();
            let idle_weapon = registry.spawn(Transform::default());
            registry
                .set_component(idle_weapon, weapon_component("weapon.test.idle"))
                .unwrap();
            (pawn_a, weapon_a, pawn_b, weapon_b, idle_weapon)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[
                remote_command(pawn_a, Some(weapon_a), 10, 5, true, false),
                remote_command(pawn_b, Some(weapon_b), 11, 5, true, false),
            ],
        );

        assert_eq!(events.authorized_shots.len(), 2);
        assert_eq!(events.weapon, vec!["activate", "activate"]);
        assert_eq!(events.authorized_shots[0].shot.pawn, pawn_a);
        assert_eq!(events.authorized_shots[1].shot.pawn, pawn_b);
        assert_ne!(
            events.authorized_shots[0].shot.shot_id,
            events.authorized_shots[1].shot.shot_id
        );
        let registry = registry.borrow();
        let weapon_a_cooldown = registry
            .get_component::<WeaponComponent>(weapon_a)
            .unwrap()
            .cooldown_remaining_ms;
        let weapon_b_cooldown = registry
            .get_component::<WeaponComponent>(weapon_b)
            .unwrap()
            .cooldown_remaining_ms;
        let idle_cooldown = registry
            .get_component::<WeaponComponent>(idle_weapon)
            .unwrap()
            .cooldown_remaining_ms;
        assert!((weapon_a_cooldown - 100.0).abs() < f32::EPSILON);
        assert!((weapon_b_cooldown - 100.0).abs() < f32::EPSILON);
        assert!((idle_cooldown - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn remote_empty_magazines_surface_each_dry_fire_without_authorizing_shots() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b) = {
            let mut registry = registry.borrow_mut();
            let (pawn_a, weapon_a) = spawn_reload_pair(&mut registry, 10, 8, 1000, 0);
            let (pawn_b, weapon_b) = spawn_reload_pair(&mut registry, 10, 8, 1000, 0);
            (pawn_a, weapon_a, pawn_b, weapon_b)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[
                remote_command(pawn_a, Some(weapon_a), 42, 9, true, false),
                remote_command(pawn_b, Some(weapon_b), 43, 9, true, false),
            ],
        );

        assert_eq!(events.weapon, vec!["dry_fire", "dry_fire"]);
        assert!(events.authorized_shots.is_empty());
        let registry = registry.borrow();
        for weapon in [weapon_a, weapon_b] {
            assert_eq!(
                registry
                    .get_component::<WeaponComponent>(weapon)
                    .unwrap()
                    .magazine,
                0
            );
        }
    }

    // Regression: the remote Auto path drained dry_fire every fixed tick while held.
    #[test]
    fn remote_empty_auto_weapon_reemits_dry_fire_only_after_cooldown() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 0);
            let mut component = registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .clone();
            component.fire_mode = FireMode::Auto;
            component.cooldown_ms = 45.0;
            registry.set_component(weapon, component).unwrap();
            (pawn, weapon)
        };

        let first = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 1, true, false)],
        );
        assert_eq!(first.weapon, vec!["dry_fire"]);
        assert!(first.authorized_shots.is_empty());

        for client_tick in [2, 3] {
            let cooling = run_remote_only_tick(
                registry.clone(),
                &[remote_command(
                    pawn,
                    Some(weapon),
                    42,
                    client_tick,
                    true,
                    false,
                )],
            );
            assert!(cooling.weapon.is_empty());
            assert!(cooling.authorized_shots.is_empty());
        }

        let ready = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 4, true, false)],
        );
        assert_eq!(ready.weapon, vec!["dry_fire"]);
        assert!(ready.authorized_shots.is_empty());
        let component = registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        assert_eq!(component.magazine, 0);
        assert!((component.cooldown_remaining_ms - 45.0).abs() <= 1.0e-5);
    }

    #[test]
    fn held_reload_starts_once_and_release_still_advances_to_completion() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 20, 1000, 2);
        let mut cooling = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        cooling.cooldown_remaining_ms = 123.0;
        registry.set_component(weapon, cooling).unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.25),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        let started = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(started.state_remaining_ms, 750);
        assert_eq!(started.state_total_ms, 1000);
        assert!(started.reload_press_consumed);
        assert_eq!(started.magazine, 2);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            20
        );

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.25),
            Vec::new()
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state_remaining_ms,
            500
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.75),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }]
        );
        let completed = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(completed.magazine, 10);
        assert_eq!(completed.state, WieldableState::Idle);
        assert_eq!(completed.state_remaining_ms, 0);
        assert_eq!(completed.state_total_ms, 0);
        assert!((completed.state_elapsed_sub_ms - 0.0).abs() < f64::EPSILON);
        assert_eq!(completed.reload_credited, 0);
        assert!((completed.cooldown_remaining_ms - 0.0).abs() < f32::EPSILON);
        assert!(!completed.reload_press_consumed);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            12
        );
    }

    #[test]
    fn reload_completion_atomically_transfers_partial_live_reserve_only_at_zero() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 3, 500, 2);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1)[0].outcome,
            ReloadOutcome::Started
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.399),
            Vec::new()
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            2
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            3
        );

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.0011),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 3 },
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            5
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            0
        );
    }

    #[test]
    fn reload_start_tick_advances_and_can_complete_immediately() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1),
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Started,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred: 8 },
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.state_total_ms, 0);
        assert_eq!(component.state_remaining_ms, 0);
        assert_eq!(component.magazine, 10);
    }

    #[test]
    fn fractional_reload_elapsed_completes_one_second_at_sixty_hz_without_drift() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);

        for tick in 0..59 {
            let deliveries =
                deliver_reload_to_weapon(&mut registry, pawn, weapon, tick == 0, 1.0 / 60.0);
            assert!(
                !deliveries
                    .iter()
                    .any(|delivery| matches!(delivery.outcome, ReloadOutcome::Completed { .. }))
            );
        }
        assert!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state_remaining_ms
                > 0
        );

        let completion = deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 1.0 / 60.0)
            .iter()
            .any(|delivery| {
                matches!(
                    delivery.outcome,
                    ReloadOutcome::Completed { transferred: 8 }
                )
            });
        assert!(completion);
    }

    #[test]
    fn reload_timer_ignores_invalid_delta_and_saturates_huge_delta() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, f32::NAN),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state_remaining_ms,
            1000
        );
        assert!(deliver_reload_to_weapon(&mut registry, pawn, weapon, false, -1.0).is_empty());
        assert!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, f32::INFINITY).is_empty()
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state_remaining_ms,
            1000
        );
        assert!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, f32::MAX)
                .iter()
                .any(|delivery| matches!(delivery.outcome, ReloadOutcome::Completed { .. }))
        );
    }

    #[test]
    fn reload_fresh_press_reports_full_and_empty_blocks_without_starting_timer() {
        let mut registry = EntityRegistry::new();
        let (full_pawn, full_weapon) = spawn_reload_pair(&mut registry, 10, 20, 900, 10);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, full_pawn, full_weapon, true, 0.1)[0].outcome,
            ReloadOutcome::BlockedFull
        );
        let full = registry
            .get_component::<WeaponComponent>(full_weapon)
            .unwrap();
        assert_eq!(full.state_remaining_ms, 0);
        assert_eq!(full.state_total_ms, 0);
        assert_eq!(full.magazine, 10);

        let (empty_pawn, empty_weapon) = spawn_reload_pair(&mut registry, 10, 0, 900, 2);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, empty_pawn, empty_weapon, true, 0.1)[0].outcome,
            ReloadOutcome::BlockedEmpty
        );
        let empty = registry
            .get_component::<WeaponComponent>(empty_weapon)
            .unwrap();
        assert_eq!(empty.state_remaining_ms, 0);
        assert_eq!(empty.state_total_ms, 0);
        assert_eq!(empty.magazine, 2);
    }

    #[test]
    fn per_shell_reload_fresh_press_reports_full_and_empty_blocks_without_starting_a_loop() {
        let mut registry = EntityRegistry::new();
        let (full_pawn, full_weapon) = spawn_reload_pair(&mut registry, 10, 20, 900, 10);
        set_reload_style(&mut registry, full_weapon, ReloadStyle::PerShell);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, full_pawn, full_weapon, true, 0.1),
            vec![ReloadDelivery {
                pawn: full_pawn,
                weapon: full_weapon,
                outcome: ReloadOutcome::BlockedFull,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(full_weapon)
                .unwrap()
                .state,
            WieldableState::Idle
        );

        let (empty_pawn, empty_weapon) = spawn_reload_pair(&mut registry, 10, 0, 900, 2);
        set_reload_style(&mut registry, empty_weapon, ReloadStyle::PerShell);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, empty_pawn, empty_weapon, true, 0.1),
            vec![ReloadDelivery {
                pawn: empty_pawn,
                weapon: empty_weapon,
                outcome: ReloadOutcome::BlockedEmpty,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(empty_weapon)
                .unwrap()
                .state,
            WieldableState::Idle
        );
    }

    #[test]
    fn reload_press_with_absent_reserve_reports_empty_without_reattaching_one() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        registry
            .remove_component::<AmmoReserve>(pawn)
            .expect("test removes the reserve before reload entry");

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::BlockedEmpty,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state,
            WieldableState::Idle
        );
        assert!(registry.get_component::<AmmoReserve>(pawn).is_err());
    }

    #[test]
    fn fresh_reload_press_mid_reload_is_silent_and_does_not_restart_timer() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 20, 1000, 2);
        assert!(!deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1).is_empty());
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            Vec::new()
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1),
            Vec::new()
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state_remaining_ms, 700);
        assert_eq!(component.state_total_ms, 1000);
    }

    #[test]
    fn reload_starts_while_cooling_without_resetting_the_cooldown() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.cooldown_remaining_ms = 90.0;
        registry.set_component(weapon, component).unwrap();

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(false, false),
            0.01,
        );
        assert_eq!(
            result.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::ShellLoading);
        assert!((component.cooldown_remaining_ms - 80.0).abs() < f32::EPSILON);
    }

    #[test]
    fn per_shell_reload_credits_one_round_per_step_and_completes_cumulatively() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 5, 4, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state,
            WieldableState::ShellLoading
        );

        for expected_magazine in [3, 4] {
            assert_eq!(
                deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
                vec![ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                }]
            );
            assert_eq!(
                registry
                    .get_component::<WeaponComponent>(weapon)
                    .unwrap()
                    .magazine,
                expected_magazine
            );
        }

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred: 3 },
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.magazine, 5);
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.reload_credited, 0);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            1
        );
    }

    #[test]
    fn per_shell_reload_status_repeats_step_progress_without_blinking_inactive() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 5, 3, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 0.0).abs() < f32::EPSILON);
        assert!(active);

        // The local HUD samples this start endpoint before the per-frame clear.
        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);
        assert!(deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.05).is_empty());
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 0.5).abs() < f32::EPSILON);
        assert!(active);
        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.05),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::ShellLoaded,
            }]
        );
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 1.0).abs() < f32::EPSILON);
        assert!(active, "a completed shell boundary stays reload-active");

        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);
        assert!(deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.05).is_empty());
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 0.5).abs() < f32::EPSILON);
        assert!(active);
        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.05),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::ShellLoaded,
            }]
        );
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 1.0).abs() < f32::EPSILON);
        assert!(
            active,
            "every per-shell boundary retains the completion sample"
        );

        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1);
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 0.0).abs() < f32::EPSILON);
        assert!(
            !active,
            "identical endpoints publish a live separator first"
        );

        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 1.0).abs() < f32::EPSILON);
        assert!(active, "the final completion endpoint remains active");

        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);
        let (progress, active) = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 0.0).abs() < f32::EPSILON);
        assert!(!active);
    }

    #[test]
    fn per_shell_overshoot_credits_multiple_steps_from_one_reserve_working_copy() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 12, 10, 50, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1515),
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
            ]
        );
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        assert_eq!(component.magazine, 5);
        assert_eq!(component.reload_credited, 3);
        assert_eq!(component.state_remaining_ms, 49);
        assert!((component.state_elapsed_sub_ms - 0.5).abs() < 0.001);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            7
        );
        assert_eq!(
            component
                .reload_feedback_sample(
                    postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
                )
                .endpoint
                .map(|endpoint| endpoint.feedback),
            Some(ReloadFeedback::Started)
        );
        component.acknowledge_reload_feedback(
            postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
        );
        let boundary = component
            .reload_feedback_sample(
                postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
            )
            .endpoint
            .expect("same-tick shell boundaries remain observable");
        assert_eq!(boundary.feedback, ReloadFeedback::Completed);
        assert_eq!(boundary.occurrences, 3);
        assert!(boundary.coalesced);
    }

    // Regression: a widened exact whole-step overshoot deferred a shell one tick.
    #[test]
    fn exact_20ms_start_tick_credits_two_10ms_shells_and_still_blocks_fire() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 10, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(true, true),
            0.02,
        );

        assert_eq!(result.authorization, WeaponFireAuthorization::Rejected);
        assert_eq!(
            result.deliveries,
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Started,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::ShellLoading);
        assert_eq!(component.state_remaining_ms, 10);
        assert!((component.state_elapsed_sub_ms - 0.0).abs() < f64::EPSILON);
        assert_eq!(component.magazine, 4);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            6
        );
    }

    #[test]
    fn per_shell_expiry_rechecks_style_and_missing_tuning_before_the_next_step() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 5, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        set_reload_style(&mut registry, weapon, ReloadStyle::Magazine);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred: 1 },
                },
            ]
        );

        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 5, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.ammo = None;
        registry.set_component(weapon, component).unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 1 },
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.state_remaining_ms, 0);
        assert!((component.state_elapsed_sub_ms - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn atomic_expiry_without_live_ammo_tuning_completes_without_transfer() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.ammo = None;
        registry.set_component(weapon, component).unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 0 },
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 2);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn descriptor_hot_reload_preserves_live_state_and_applies_style_at_expiry() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);

        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.refresh_from_descriptor(&refreshed_ammo_descriptor(ReloadStyle::Magazine));
        assert_eq!(component.state, WieldableState::ShellLoading);
        assert_eq!(component.state_remaining_ms, 100);
        assert_eq!(component.reload_credited, 0);
        registry.set_component(weapon, component).unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred: 1 },
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 3);

        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.refresh_from_descriptor(&refreshed_ammo_descriptor(ReloadStyle::PerShell));
        assert_eq!(component.state, WieldableState::Reloading);
        assert_eq!(component.state_remaining_ms, 100);
        registry.set_component(weapon, component).unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 10);
    }

    #[test]
    fn accepted_fire_cancels_per_shell_reload_and_keeps_credited_rounds() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.cooldown_remaining_ms = 10.0;
        registry.set_component(weapon, component).unwrap();

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.01,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Accepted);
        assert_eq!(
            result.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Cancelled { transferred: 0 },
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(
            component.magazine, 1,
            "the accepted shot spends after cancel"
        );
        assert_eq!(component.reload_credited, 0);
        assert!(!component.reload_press_consumed);
        assert!(
            component
                .reload_feedback_sample(
                    postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
                )
                .endpoint
                .is_none()
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn held_reload_restarts_a_per_shell_loop_on_the_tick_after_cancel() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);

        let started = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(false, false),
            0.0,
        );
        assert_eq!(
            started.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );

        let cancelled = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(cancelled.authorization, WeaponFireAuthorization::Accepted);
        assert_eq!(
            cancelled.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Cancelled { transferred: 0 },
            }]
        );
        assert!(
            !registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_press_consumed
        );

        let restarted = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(false, false),
            0.0,
        );
        assert_eq!(
            restarted.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state,
            WieldableState::ShellLoading
        );
    }

    #[test]
    fn releasing_held_reload_after_shell_cancel_leaves_loaded_rounds_idle() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(false, false),
            0.1,
        );

        let cancelled = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            true,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(cancelled.authorization, WeaponFireAuthorization::Accepted);
        assert_eq!(
            cancelled.deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Cancelled { transferred: 1 },
            }]
        );

        let released = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(false, false),
            0.0,
        );
        assert!(released.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 2);
        assert!(!component.reload_press_consumed);
        assert!(
            component
                .reload_feedback_sample(
                    postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
                )
                .endpoint
                .is_none(),
            "a later cancellation clears prior-tick completion feedback"
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            7
        );
    }

    #[test]
    fn shell_expiry_then_fire_cancels_only_the_restarted_step() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.1,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Accepted);
        assert_eq!(
            result.deliveries,
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Cancelled { transferred: 1 },
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(
            component.magazine, 2,
            "one credited shell then one fired shot"
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            7
        );
        let endpoint = component
            .reload_feedback_sample(
                postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
            )
            .endpoint
            .expect("same-tick completion survives cancellation");
        assert_eq!(endpoint.feedback, ReloadFeedback::Completed);
        assert_eq!(endpoint.occurrences, 1);
    }

    #[test]
    fn suppressed_shell_fire_keeps_the_loop_and_does_not_emit_dry_fire() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 0);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.ammo.as_mut().unwrap().cost_per_shot = 2;
        registry.set_component(weapon, component).unwrap();
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.0,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Rejected);
        assert!(result.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::ShellLoading);
        assert!((component.cooldown_remaining_ms - 0.0).abs() < f32::EPSILON);
        assert_eq!(component.magazine, 0);
    }

    #[test]
    fn cooldown_suppressed_shell_fire_only_applies_normal_decrement() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.cooldown_remaining_ms = 75.0;
        registry.set_component(weapon, component).unwrap();
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);

        // Regression: a cooldown-suppressed pull took the Empty path, cancelling
        // the loop and resetting cooldown instead of applying only the tick decrement.
        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.025,
        );

        assert_eq!(result.authorization, WeaponFireAuthorization::Rejected);
        assert!(result.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::ShellLoading);
        assert!((component.cooldown_remaining_ms - 50.0).abs() < f32::EPSILON);
        assert_eq!(component.magazine, 2);
    }

    #[test]
    fn per_shell_take_zero_guard_completes_without_a_shell_event() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 5, 100, 4);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.state = WieldableState::ShellLoading;
        component.state_remaining_ms = 0;
        component.state_total_ms = 100;
        component.reload_credited = 2;
        let feedback_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
        registry.set_component(weapon, component).unwrap();
        registry.set_component(pawn, AmmoReserve::new()).unwrap();

        // This fabricated state covers the defensive take -> 0 path: ordinary
        // loop continuation already rejects an empty working reserve.
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.0),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 2 },
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 4);
        assert_eq!(component.reload_credited, 0);
        assert_eq!(component.state_remaining_ms, 0);
        assert!((component.state_elapsed_sub_ms - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn per_shell_final_credit_completes_before_same_tick_fire() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 3, 1, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        crate::sim::clear_reload_feedback_for_weapon(&mut registry, weapon);

        let result = tick_machine(
            &mut registry,
            Some(pawn),
            weapon,
            false,
            &fire_command(true, true),
            0.1,
        );
        assert_eq!(result.authorization, WeaponFireAuthorization::Accepted);
        assert_eq!(
            result.deliveries,
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred: 1 },
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(
            component.magazine, 2,
            "completion refills before the shot spends"
        );
        assert_eq!(
            component
                .reload_feedback_sample(
                    postretro_entities::components::weapon::ReloadFeedbackConsumer::Hud,
                )
                .endpoint
                .map(|endpoint| endpoint.feedback),
            Some(ReloadFeedback::Completed)
        );
    }

    #[test]
    fn pawnless_shell_expiry_ends_silently_without_debiting_the_reserve() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);

        let result = tick_machine(
            &mut registry,
            None,
            weapon,
            false,
            &fire_command(false, false),
            0.1,
        );
        assert!(result.deliveries.is_empty());
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.state_remaining_ms, 0);
        assert_eq!(component.state_total_ms, 0);
        assert_eq!(component.reload_credited, 0);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn shell_expiry_with_absent_reserve_completes_without_reattaching_one() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
        set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
        let _ = deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.0);
        registry
            .remove_component::<AmmoReserve>(pawn)
            .expect("test removes the reserve after reload entry");

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 0 },
            }]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 2);
        assert!(
            registry.get_component::<AmmoReserve>(pawn).is_err(),
            "expiry must not materialize an absent pawn reserve"
        );
    }

    #[test]
    fn resourceless_weapon_cannot_reload_and_release_clears_edge_state() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(weapon, weapon_component("weapon.test.unlimited"))
            .unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1),
            Vec::new()
        );
        assert!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_press_consumed
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            Vec::new()
        );
        assert!(
            !registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_press_consumed
        );
    }

    #[test]
    fn local_reload_routes_to_local_pawn_reserve_before_fire() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();

        let events = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            Some(weapon),
            0.0,
            &mut progress,
            &mut ai_runtime,
            &[],
            &mut mover_states,
            &[],
            &sim_command(true, true),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            0.25,
            None,
            |_| {},
        );

        assert_eq!(
            events.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        assert!(
            events.weapon.is_empty(),
            "reload start must block same-tick fire"
        );
        let registry = registry.borrow();
        let weapon = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(weapon.state_remaining_ms, 750);
        assert_eq!(weapon.magazine, 2);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn immediate_local_reload_still_blocks_fire_for_start_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 10, 2);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };
        let events = run_local_only_tick(
            registry.clone(),
            weapon,
            &sim_command(true, true),
            1.0 / 60.0,
        );

        assert!(events.weapon.is_empty());
        assert!(events.reload_deliveries.iter().any(|delivery| {
            delivery.pawn == pawn && delivery.outcome == ReloadOutcome::Started
        }));
        assert!(events.reload_deliveries.iter().any(|delivery| {
            delivery.pawn == pawn
                && matches!(
                    delivery.outcome,
                    ReloadOutcome::Completed { transferred: 8 }
                )
        }));
        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            10
        );
        let (progress, active) = registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 0.0).abs() < f32::EPSILON);
        assert!(active);

        // Regression: a start and completion produced during one short catch-up
        // tick must reach the frame consumer as two ordered publications.
        crate::sim::clear_reload_feedback_for_weapon(&mut registry.borrow_mut(), weapon);
        let (progress, active) = registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_status();
        assert!((progress - 1.0).abs() < f32::EPSILON);
        assert!(active);
    }

    #[test]
    fn local_reload_completion_refills_before_same_tick_fire() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };

        let started =
            run_local_only_tick(registry.clone(), weapon, &sim_command(false, true), 0.04);
        assert_eq!(
            started.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        let advancing =
            run_local_only_tick(registry.clone(), weapon, &sim_command(false, false), 0.04);
        assert!(advancing.reload_deliveries.is_empty());

        // Completion is not a new reload start: transfer settles before fire
        // authorization, so this tick may spend from the refilled magazine.
        let completed_and_fired =
            run_local_only_tick(registry.clone(), weapon, &sim_command(true, false), 0.021);
        assert_eq!(
            completed_and_fired.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }]
        );
        assert_eq!(completed_and_fired.weapon, vec!["activate"]);
        let registry = registry.borrow();
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state_remaining_ms, 0);
        assert_eq!(component.magazine, 9);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            0
        );
    }

    #[test]
    fn immediate_remote_reload_still_blocks_fire_for_start_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            spawn_reload_pair(&mut registry, 10, 8, 10, 2)
        };

        let events = run_remote_only_tick(
            registry,
            &[remote_command(pawn, Some(weapon), 42, 9, true, true)],
        );

        assert!(events.authorized_shots.is_empty());
        assert!(events.weapon.is_empty());
        assert!(
            events
                .reload_deliveries
                .iter()
                .any(|delivery| { delivery.outcome == ReloadOutcome::Started })
        );
        assert!(events.reload_deliveries.iter().any(|delivery| {
            matches!(
                delivery.outcome,
                ReloadOutcome::Completed { transferred: 8 }
            )
        }));
    }

    #[test]
    fn local_per_shell_reload_start_blocks_fire_then_an_authorized_shot_cancels() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };

        let started = run_local_only_tick(registry.clone(), weapon, &sim_command(true, true), 0.0);
        assert!(started.weapon.is_empty());
        assert_eq!(
            started.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );

        // The start-tick latch still performs the normal semi-edge bookkeeping,
        // so release before the next press that is allowed to cancel the loop.
        let released =
            run_local_only_tick(registry.clone(), weapon, &sim_command(false, false), 0.0);
        assert!(released.weapon.is_empty());
        assert!(released.reload_deliveries.is_empty());

        let cancelled =
            run_local_only_tick(registry.clone(), weapon, &sim_command(true, false), 0.0);
        assert_eq!(cancelled.weapon, vec!["activate"]);
        assert_eq!(
            cancelled.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Cancelled { transferred: 0 },
            }]
        );
        let registry = registry.borrow();
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 1);
    }

    #[test]
    fn remote_per_shell_authorization_runs_the_same_cancel_machine() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            (pawn, weapon)
        };

        let started = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 1, false, true)],
        );
        assert!(started.authorized_shots.is_empty());
        assert_eq!(
            started.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );

        let cancelled = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 2, true, false)],
        );
        assert_eq!(cancelled.authorized_shots.len(), 1);
        assert_eq!(cancelled.weapon, vec!["activate"]);
        assert_eq!(
            cancelled.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Cancelled { transferred: 0 },
            }]
        );
        let registry = registry.borrow();
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.magazine, 1);
    }

    #[test]
    fn remote_weapon_without_shot_id_cannot_cancel_per_shell_reload() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            (pawn, weapon)
        };
        let _ = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 1, false, true)],
        );
        let mut command = remote_command(pawn, Some(weapon), 42, 2, true, false);
        command.shot_id = None;

        let refused = run_remote_only_tick(registry.clone(), &[command]);

        assert!(refused.authorized_shots.is_empty());
        assert!(refused.weapon.is_empty());
        assert!(refused.reload_deliveries.is_empty());
        let registry = registry.borrow();
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.state, WieldableState::ShellLoading);
        assert_eq!(component.magazine, 2);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn remote_per_shell_matches_local_credit_and_reserve_counts() {
        let local_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (local_pawn, local_weapon) = {
            let mut registry = local_registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 1);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };
        let remote_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (remote_pawn, remote_weapon) = {
            let mut registry = remote_registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 1);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            (pawn, weapon)
        };

        let _ = run_local_only_tick(
            local_registry.clone(),
            local_weapon,
            &sim_command(false, true),
            1.0 / 60.0,
        );
        let _ = run_remote_only_tick(
            remote_registry.clone(),
            &[remote_command(
                remote_pawn,
                Some(remote_weapon),
                42,
                1,
                false,
                true,
            )],
        );
        for tick in 2..=6 {
            let _ = run_local_only_tick(
                local_registry.clone(),
                local_weapon,
                &sim_command(false, false),
                1.0 / 60.0,
            );
            let _ = run_remote_only_tick(
                remote_registry.clone(),
                &[remote_command(
                    remote_pawn,
                    Some(remote_weapon),
                    42,
                    tick,
                    false,
                    false,
                )],
            );
        }

        let local = local_registry.borrow();
        let remote = remote_registry.borrow();
        let local_weapon_component = local
            .get_component::<WeaponComponent>(local_weapon)
            .unwrap();
        let remote_weapon_component = remote
            .get_component::<WeaponComponent>(remote_weapon)
            .unwrap();
        assert_eq!(local_weapon_component.magazine, 2);
        assert_eq!(
            local_weapon_component.magazine,
            remote_weapon_component.magazine
        );
        assert_eq!(
            local
                .get_component::<AmmoReserve>(local_pawn)
                .unwrap()
                .available("bullets.light"),
            remote
                .get_component::<AmmoReserve>(remote_pawn)
                .unwrap()
                .available("bullets.light")
        );
    }

    #[test]
    fn remote_host_refuses_predicted_shots_during_magazine_and_under_cost_shell_reloads() {
        let magazine_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (magazine_pawn, magazine_weapon) = {
            let mut registry = magazine_registry.borrow_mut();
            spawn_reload_pair(&mut registry, 10, 8, 100, 2)
        };
        let _ = run_remote_only_tick(
            magazine_registry.clone(),
            &[remote_command(
                magazine_pawn,
                Some(magazine_weapon),
                42,
                1,
                false,
                true,
            )],
        );
        let magazine_refusal = run_remote_only_tick(
            magazine_registry.clone(),
            &[remote_command(
                magazine_pawn,
                Some(magazine_weapon),
                42,
                2,
                true,
                false,
            )],
        );
        assert!(magazine_refusal.authorized_shots.is_empty());
        assert!(magazine_refusal.weapon.is_empty());
        assert_eq!(
            magazine_registry
                .borrow()
                .get_component::<WeaponComponent>(magazine_weapon)
                .unwrap()
                .state,
            WieldableState::Reloading
        );

        let shell_registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (shell_pawn, shell_weapon) = {
            let mut registry = shell_registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 1);
            set_reload_style(&mut registry, weapon, ReloadStyle::PerShell);
            let mut component = registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .clone();
            component.ammo.as_mut().unwrap().cost_per_shot = 2;
            registry.set_component(weapon, component).unwrap();
            (pawn, weapon)
        };
        let _ = run_remote_only_tick(
            shell_registry.clone(),
            &[remote_command(
                shell_pawn,
                Some(shell_weapon),
                42,
                1,
                false,
                true,
            )],
        );
        let shell_refusal = run_remote_only_tick(
            shell_registry.clone(),
            &[remote_command(
                shell_pawn,
                Some(shell_weapon),
                42,
                2,
                true,
                false,
            )],
        );
        assert!(shell_refusal.authorized_shots.is_empty());
        assert!(shell_refusal.weapon.is_empty());
        assert!(shell_refusal.reload_deliveries.is_empty());
        assert_eq!(
            shell_registry
                .borrow()
                .get_component::<WeaponComponent>(shell_weapon)
                .unwrap()
                .state,
            WieldableState::ShellLoading
        );
    }

    #[test]
    fn remote_reload_delivery_routes_to_mapped_weapon_and_pawn_reserve_only() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b) = {
            let mut registry = registry.borrow_mut();
            let (pawn_a, weapon_a) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);
            let (pawn_b, weapon_b) = spawn_reload_pair(&mut registry, 10, 20, 1000, 4);
            (pawn_a, weapon_a, pawn_b, weapon_b)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[
                remote_command(pawn_a, Some(weapon_a), 10, 5, false, true),
                remote_command(pawn_b, Some(weapon_b), 11, 5, false, false),
            ],
        );

        assert_eq!(
            events.reload_deliveries,
            vec![ReloadDelivery {
                pawn: pawn_a,
                weapon: weapon_a,
                outcome: ReloadOutcome::Started,
            }]
        );
        let registry = registry.borrow();
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn_a)
                .unwrap()
                .available("bullets.light"),
            8
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn_b)
                .unwrap()
                .available("bullets.light"),
            20
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_b)
                .unwrap()
                .state_remaining_ms,
            0
        );
    }

    #[test]
    fn local_pellet_policy_settles_before_a_despawned_target_can_receive_later_pellets() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, target) = {
            let mut registry = registry.borrow_mut();
            let (pawn, _) = spawn_local_pellet_weapon(&mut registry, "weapon.test.pellet");
            let target = spawn_pellet_target(&mut registry);
            (pawn, target)
        };
        let mut policy_fires = 0;
        let mut policy = |registry: &mut EntityRegistry| {
            policy_fires += 1;
            if registry.exists(target) {
                registry.despawn(target).unwrap();
            }
        };

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(true, true),
            false,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut policy,
        );

        assert_eq!(result.weapon_events, vec!["activate", "impact"]);
        assert_eq!(
            result.weapon_impact_points.len(),
            8,
            "the cast set includes skipped pellets"
        );
        assert_eq!(policy_fires, 1, "target despawn settles before pellet two");
        assert!(!registry.borrow().exists(target));
    }

    #[test]
    fn local_pellet_policy_settles_before_a_despawned_shooter_can_fire_later_policies() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, target) = {
            let mut registry = registry.borrow_mut();
            let (pawn, _) = spawn_local_pellet_weapon(&mut registry, "weapon.test.pellet");
            let target = spawn_pellet_target(&mut registry);
            (pawn, target)
        };
        let mut policy_fires = 0;
        let mut policy = |registry: &mut EntityRegistry| {
            policy_fires += 1;
            if registry.exists(pawn) {
                registry.despawn(pawn).unwrap();
            }
        };

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(true, true),
            false,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut policy,
        );

        assert_eq!(
            result.weapon_impact_points.len(),
            8,
            "the cast set retains every pellet"
        );
        assert_eq!(policy_fires, 1, "shooter despawn settles before pellet two");
        let health = registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        assert!((health.current - 90.0).abs() < f32::EPSILON);
    }

    #[test]
    fn local_world_pellets_run_policy_once_per_impact_without_damage() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let pawn = {
            let mut registry = registry.borrow_mut();
            spawn_local_pellet_weapon(&mut registry, "weapon.test.pellet").0
        };
        let mut policy_fires = 0;
        let mut policy = |_: &mut EntityRegistry| policy_fires += 1;

        let result = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(true, true),
            false,
            &wall_world(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut policy,
        );

        assert_eq!(result.weapon_events, vec!["activate", "impact"]);
        assert_eq!(result.weapon_impact_points.len(), 8);
        assert_eq!(
            policy_fires, 8,
            "each world pellet runs its policy sequence"
        );
    }

    #[test]
    fn local_pellet_policy_swap_keeps_the_outgoing_weapon_credit_for_the_whole_shell() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, target) = {
            let mut registry = registry.borrow_mut();
            let (pawn, _) = spawn_local_pellet_weapon(&mut registry, "weapon.test.outgoing");
            let incoming = registry.spawn(Transform::default());
            registry
                .set_component(incoming, weapon_component("weapon.test.incoming"))
                .unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(incoming);
            registry.set_component(pawn, inventory).unwrap();
            let target = spawn_pellet_target(&mut registry);
            (pawn, target)
        };
        let mut swapped = false;
        let mut policy = |registry: &mut EntityRegistry| {
            if !swapped {
                let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
                inventory.active_slot = 1;
                registry.set_component(pawn, inventory).unwrap();
                swapped = true;
            }
        };

        let _ = run_local_weapon_command(
            &registry,
            Some(pawn),
            false,
            None,
            &fire_command(true, true),
            false,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            0.0,
            &mut policy,
        );

        let health = registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        assert_eq!(health.contributor_ledger.total_recorded_hits(), 8);
        assert_eq!(
            health
                .contributor_ledger
                .recorded_damage_by_source("weapon.test.outgoing"),
            Some(80.0)
        );
        assert_eq!(
            health
                .contributor_ledger
                .recorded_damage_by_source("weapon.test.incoming"),
            None
        );
    }

    #[test]
    fn weapon_impact_damage_records_effective_source_weapon_zone_and_scaled_payload() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        registry
            .set_component(weapon_id, weapon_component("weapon.test.rifle"))
            .unwrap();

        let target = registry.spawn(Transform::default());
        let mut health = HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: Default::default(),
            contributor_ledger: Default::default(),
        };
        health.zone_multipliers.insert("head".to_string(), 2.5);
        registry.set_component(target, health).unwrap();

        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: Some("head".to_string()),
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload { amount: 10.0 }),
        };

        let attacker = registry.spawn(Transform::default());
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon_id);
        registry.set_component(attacker, inventory).unwrap();
        apply_weapon_impact_damage(&mut registry, Some(attacker), &impact);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert!((health.current - 75.0).abs() < f32::EPSILON);
        let entries = health.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.source_id, "weapon.test.rifle");
        assert!((entry.accumulated_damage - 25.0).abs() < f32::EPSILON);
        assert!((entry.last_hit_damage - 25.0).abs() < f32::EPSILON);
        assert_eq!(entry.last_hit_zone.as_deref(), Some("head"));
        assert_eq!(entry.last_weapon, Some(weapon_id));
        assert_eq!(entry.last_attacker, Some(attacker));
    }

    #[test]
    fn weapon_impact_damage_skips_non_finite_scaled_payload() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        registry
            .set_component(weapon_id, weapon_component("weapon.test.rifle"))
            .unwrap();

        let target = registry.spawn(Transform::default());
        let mut health = HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: Default::default(),
            contributor_ledger: Default::default(),
        };
        health.zone_multipliers.insert("over".to_string(), 2.0);
        registry.set_component(target, health).unwrap();

        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: Some("over".to_string()),
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload { amount: f32::MAX }),
        };

        let attacker = registry.spawn(Transform::default());
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon_id);
        registry.set_component(attacker, inventory).unwrap();
        apply_weapon_impact_damage(&mut registry, Some(attacker), &impact);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert!((health.current - 100.0).abs() < f32::EPSILON);
        assert!(health.contributor_ledger.entries().is_empty());
        assert!(health.contributor_ledger.overflow().is_none());
    }
}
