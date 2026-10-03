// Each real command advances central state once and freezes every due shot.
// See: context/lib/networking.md §Combat authority · context/lib/entity_model.md §5
use super::reconcile::ActivationRecords;
use crate::{sim, weapon};
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{ActivationToken, WeaponPlacementDescriptor};

pub(crate) struct QueuedShot {
    pub component: postretro_entities::components::weapon::WeaponComponent,
    pub pawn: EntityId,
    pub weapon: EntityId,
    pub slot: usize,
    pub pellet_salt: String,
    pub placement: WeaponPlacementDescriptor,
    pub shot: weapon::ResolvedWeaponShot,
    pub presentation: weapon::ClientPullPresentation,
    pub cooldown_before: f32,
    pub cooldown_after: f32,
}
#[derive(Default)]
pub(crate) struct ClientWeaponFrame {
    pub due: Vec<QueuedShot>,
    pub records: ActivationRecords,
    pub suppressed: Option<(EntityId, ActivationToken)>,
}
impl ClientWeaponFrame {
    pub fn clear(&mut self) {
        self.due.clear();
        self.records.clear();
        self.suppressed = None;
    }
    pub fn suspend(&mut self, registry: &EntityRegistry) {
        if let Some((_, id)) = crate::local_active_wieldable(registry)
            && let Ok(component) = registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(id)
            && let Some(cursor) = component.state.activation_cursor()
        {
            self.suppressed = Some((id, cursor.token));
        }
    }
    /// Owner-private death is a lifecycle fact, independent of local health
    /// simulation. Already produced shots and flights retain their semantics.
    pub fn apply_owner_liveness(
        &mut self,
        registry: &mut EntityRegistry,
        slots: &postretro_entities::SlotTable,
        capture: &mut crate::input::ActivationInputCapture,
    ) -> bool {
        let alive = !slots.get("player.health").is_some_and(|slot| {
            matches!(slot.value, Some(postretro_entities::SlotValue::Number(health)) if health <= 0.0 || !health.is_finite())
        });
        if !alive
            && let Some((_, weapon)) = crate::local_active_wieldable(registry)
            && let Ok(ComponentValue::Weapon(component)) =
                registry.get_component_value_mut(weapon, ComponentKind::Weapon)
            && let Some(cursor) = component.state.activation_cursor()
        {
            let token = cursor.token;
            component.cancel_activation();
            self.records.local_terminal(token);
            capture.terminal(token);
            if self.suppressed == Some((weapon, token)) {
                self.suppressed = None;
            }
        }
        alive
    }
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn predict(
        &mut self,
        registry: &mut EntityRegistry,
        command: &mut sim::SimCommand,
        tick: u32,
        pawn_network: u32,
        dt_ms: f32,
        placement: WeaponPlacementDescriptor,
        projection: &weapon::ReplicatedWeaponProjection,
    ) -> Option<ActivationToken> {
        self.predict_with_liveness(
            registry,
            command,
            tick,
            pawn_network,
            dt_ms,
            placement,
            projection,
            true,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn predict_with_liveness(
        &mut self,
        registry: &mut EntityRegistry,
        command: &mut sim::SimCommand,
        tick: u32,
        pawn_network: u32,
        dt_ms: f32,
        placement: WeaponPlacementDescriptor,
        projection: &weapon::ReplicatedWeaponProjection,
        owner_alive: bool,
    ) -> Option<ActivationToken> {
        self.records.advance_time(dt_ms);
        let pawn = registry.local_player_movement_pawn()?;
        let (slot, id) = crate::local_active_wieldable(registry)?;
        let pellet_salt = {
            let component = registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(id)
                .ok()?;
            std::sync::Arc::clone(&component.descriptor_identity)
        };
        command.firing_slot = u8::try_from(slot).ok()?;
        let alive = owner_alive
            && registry.exists(pawn)
            && !postretro_sim::scripting_systems::health::is_terminally_committed_to_removal(
                registry, pawn,
            )
            && !registry
                .get_component::<postretro_entities::components::health::HealthComponent>(pawn)
                .is_ok_and(|health| health.current <= 0.0 || !health.current.is_finite());
        let ComponentValue::Weapon(component) = registry
            .get_component_value_mut(id, ComponentKind::Weapon)
            .ok()?
        else {
            return None;
        };
        if !alive && let Some(cursor) = component.state.activation_cursor() {
            self.records.local_terminal(cursor.token);
            component.cancel_activation();
        }
        if let Some(token) = command.activation.initiation {
            self.records.request(token, id);
        }
        let before = component.cooldown_remaining_ms;
        let advanced = weapon::activation_prediction::advance_predicted_weapon_tick(
            component,
            weapon::execution::ActivationCommand {
                tick,
                pawn: pawn_network,
                real_command: true,
                input: command.activation,
                controller_starts: true,
                primary: command.fire_button,
                secondary: command.secondary_button,
            },
            command.reload,
            dt_ms,
            alive,
        );
        if let Some(token) = advanced.initiated {
            command.activation.initiation = Some(token);
            self.records.request(token, id);
        }
        if let Some(token) = advanced.rejected {
            self.records.local_terminal(token);
        }
        if let Some((token, _)) = advanced.terminal {
            self.records.local_terminal(token);
        }
        if let Some(attempt) = advanced.shot {
            let mut shot = weapon::freeze_weapon_shot(component, attempt);
            if !self.records.correct_new_shot(&mut shot) {
                component.cancel_activation();
                self.records
                    .local_terminal(postretro_foundation::ActivationToken {
                        start_tick: shot.activation.shot_id.start_tick,
                        lane: shot.activation.shot_id.lane,
                    });
                return advanced.initiated.or(command.activation.initiation);
            }
            let presentation = weapon::client_shot_presentation(component, slot, projection, &shot);
            self.records.retain_shot(id, &shot, presentation);
            self.due.push(QueuedShot {
                component: component.clone(),
                pawn,
                weapon: id,
                slot,
                pellet_salt: pellet_salt.to_string(),
                placement,
                shot,
                presentation,
                cooldown_before: before,
                cooldown_after: component.cooldown_remaining_ms,
            });
        }
        if self
            .suppressed
            .is_some_and(|(weapon, token)| weapon == id && command.activation.cancel == Some(token))
        {
            self.suppressed = None;
        }
        advanced.initiated.or(command.activation.initiation)
    }
}
