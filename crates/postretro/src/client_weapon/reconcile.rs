// Correlated activation outcomes bind captured local instances to host identities.
// See: context/lib/networking.md §Combat authority · context/lib/entity_model.md §5
use crate::weapon;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::ActivationToken;
use std::collections::{HashMap, VecDeque};

const MAX_RECORDS: usize = 64;
struct ActivationRecord {
    weapon: EntityId,
    host_weapon: Option<u32>,
    charge: Option<f32>,
    age_ms: f32,
    terminal: bool,
    invalid_correction: bool,
    shots: Vec<(weapon::ResolvedWeaponShot, weapon::ClientPullPresentation)>,
}
pub(crate) struct OutcomeEffect {
    pub token: ActivationToken,
    pub weapon: EntityId,
    pub terminal: bool,
    pub rejected: bool,
    pub recovery_ms: Option<f32>,
}
#[derive(Default)]
pub(crate) struct ActivationRecords {
    records: HashMap<ActivationToken, ActivationRecord>,
    order: VecDeque<ActivationToken>,
    latest: HashMap<EntityId, ActivationToken>,
}
impl ActivationRecords {
    pub fn clear(&mut self) {
        self.records.clear();
        self.order.clear();
        self.latest.clear();
    }
    pub fn advance_time(&mut self, dt_ms: f32) {
        for record in self.records.values_mut() {
            record.age_ms += dt_ms.max(0.0);
        }
        self.records
            .retain(|_, record| !record.terminal || record.age_ms <= 122_000.0);
        self.order.retain(|token| self.records.contains_key(token));
        self.latest
            .retain(|_, token| self.records.contains_key(token));
    }
    pub fn request(&mut self, token: ActivationToken, weapon: EntityId) {
        if self.records.contains_key(&token) {
            return;
        }
        if self.records.len() == MAX_RECORDS {
            if let Some(index) = self
                .order
                .iter()
                .position(|token| self.records.get(token).is_some_and(|r| r.terminal))
            {
                if let Some(old) = self.order.remove(index) {
                    self.records.remove(&old);
                }
            } else {
                return;
            }
        }
        self.records.insert(
            token,
            ActivationRecord {
                weapon,
                host_weapon: None,
                charge: None,
                age_ms: 0.0,
                terminal: false,
                invalid_correction: false,
                shots: Vec::new(),
            },
        );
        self.order.push_back(token);
        self.latest.insert(weapon, token);
    }
    pub fn local_terminal(&mut self, token: ActivationToken) {
        if let Some(record) = self.records.get_mut(&token)
            && !record.terminal
        {
            record.age_ms = 0.0;
            record.terminal = true;
        }
    }
    pub fn correct_new_shot(&self, shot: &mut weapon::ResolvedWeaponShot) -> bool {
        let id = shot.activation.shot_id;
        if let Some(record) = self.records.get(&ActivationToken {
            start_tick: id.start_tick,
            lane: id.lane,
        }) {
            if record.invalid_correction {
                return false;
            }
            if let Some(charge) = record.charge {
                match shot.with_authoritative_charge(charge) {
                    Ok(corrected) => *shot = corrected,
                    Err(_) => return false,
                }
            }
        }
        true
    }
    pub fn retain_shot(
        &mut self,
        weapon: EntityId,
        shot: &weapon::ResolvedWeaponShot,
        presentation: weapon::ClientPullPresentation,
    ) {
        let id = shot.activation.shot_id;
        if let Some(record) = self.records.get_mut(&ActivationToken {
            start_tick: id.start_tick,
            lane: id.lane,
        }) && record.weapon == weapon
            && record.shots.len() < 16
        {
            record.shots.push((shot.clone(), presentation));
        }
    }
    pub fn outcome(
        &mut self,
        registry: &mut EntityRegistry,
        outcome: postretro_net::wire::ActivationOutcome,
    ) -> Option<OutcomeEffect> {
        use postretro_net::wire::ActivationOutcome as O;
        let wire_token = match outcome {
            O::InitiationAccepted { token, .. }
            | O::InitiationRejected { token, .. }
            | O::ExecutionAccepted { token, .. }
            | O::Cancelled { token, .. }
            | O::Completed { token, .. } => token,
        };
        let lane = postretro_foundation::ActivationLane::from_tag(wire_token.lane)?;
        let token = ActivationToken {
            start_tick: wire_token.start_tick,
            lane,
        };
        let record = self.records.get_mut(&token)?;
        if registry
            .get_component::<WeaponComponent>(record.weapon)
            .is_err()
        {
            return None;
        }
        let (host, charge, recovery, mut cancel) = match outcome {
            O::InitiationAccepted { weapon, .. } => {
                if record.host_weapon.is_none() {
                    record.host_weapon = Some(weapon.0);
                }
                return None;
            }
            O::InitiationRejected { recovery_ticks, .. } => {
                (None, None, Some(recovery_ticks), true)
            }
            O::ExecutionAccepted {
                weapon,
                charge_millionths,
                recovery_ticks,
                ..
            } => (
                Some(weapon.0),
                Some(charge_millionths as f32 / 1_000_000.0),
                Some(recovery_ticks),
                false,
            ),
            O::Cancelled {
                weapon,
                recovery_ticks,
                ..
            } => (Some(weapon.0), None, Some(recovery_ticks), true),
            O::Completed {
                weapon,
                recovery_ticks,
                ..
            } => (Some(weapon.0), None, Some(recovery_ticks), true),
        };
        if host.is_some() && host != record.host_weapon {
            return None;
        }
        if charge.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            return None;
        }
        if let Some(charge) = charge {
            record.charge = Some(charge);
            for (shot, _) in &mut record.shots {
                match shot.with_authoritative_charge(charge) {
                    Ok(corrected) => {
                        crate::sim::correct_predicted_projectile(registry, shot, &corrected);
                        *shot = corrected;
                    }
                    Err(_) => {
                        record.invalid_correction = true;
                        cancel = true;
                        break;
                    }
                }
            }
        }
        let mut recovery_ms = None;
        if let Ok(postretro_entities::ComponentValue::Weapon(component)) = registry
            .get_component_value_mut(record.weapon, postretro_entities::ComponentKind::Weapon)
        {
            if component
                .state
                .activation_cursor()
                .is_some_and(|cursor| cursor.token == token)
            {
                if cancel {
                    component.cancel_activation();
                } else if let Some(charge) = charge
                    && let postretro_entities::components::wieldable_state::WieldableState::Executing(mut cursor) = component.state { cursor.charge = charge; component.state = postretro_entities::components::wieldable_state::WieldableState::Executing(cursor); }
            }
            if self.latest.get(&record.weapon) == Some(&token)
                && let Some(ticks) = recovery
            {
                let host_ms = ticks as f32 * (1000.0 / 60.0);
                // The host's remaining recovery is one transit stale. An admitted
                // execution's recovery began at this client's own shot, which is
                // where host admission measures cadence from, so it may only
                // shorten the local countdown. A refused start adopts the host's,
                // which never runs ahead of admission.
                let applied = if matches!(outcome, O::InitiationRejected { .. }) {
                    host_ms
                } else {
                    host_ms.min(component.cooldown_remaining_ms)
                };
                component.cooldown_remaining_ms = applied;
                recovery_ms = Some(applied);
            }
        }
        let terminal = cancel || matches!(outcome, O::Completed { .. });
        if terminal && !record.terminal {
            record.age_ms = 0.0;
            record.terminal = true;
        }
        Some(OutcomeEffect {
            token,
            weapon: record.weapon,
            terminal,
            rejected: record.invalid_correction || matches!(outcome, O::InitiationRejected { .. }),
            recovery_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_net::wire::{ActivationOutcome as O, NetworkId, WireActivationToken};

    fn cooldown(registry: &EntityRegistry, weapon: EntityId) -> f32 {
        registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .cooldown_remaining_ms
    }

    // Regression: an admitted execution's outcome reset the client's recovery to
    // the host's transit-stale remaining value, so the client fired slower than
    // the host player.
    #[test]
    fn client_weapon_admitted_outcome_only_shortens_recovery_and_refusal_adopts_host() {
        let descriptor: postretro_foundation::WeaponDescriptor =
            serde_json::from_value(serde_json::json!({
                "damage": 5, "range": 20, "resolution": "hitscan",
                "primary": { "trigger": "hold", "recoveryMs": 130, "steps": [{ "kind": "shot" }] },
            }))
            .unwrap();
        let mut registry = EntityRegistry::new();
        let weapon = registry.spawn(postretro_entities::Transform::default());
        let mut component = WeaponComponent::from_descriptor(&descriptor.validate().unwrap());
        component.cooldown_remaining_ms = 100.0;
        registry.set_component(weapon, component).unwrap();
        let mut records = ActivationRecords::default();
        let token = ActivationToken {
            start_tick: 10,
            lane: postretro_foundation::ActivationLane::Primary,
        };
        let wire = WireActivationToken {
            start_tick: 10,
            lane: 0,
        };
        records.request(token, weapon);
        records.outcome(
            &mut registry,
            O::InitiationAccepted {
                token: wire,
                weapon: NetworkId(3),
            },
        );
        let completed = records
            .outcome(
                &mut registry,
                O::Completed {
                    token: wire,
                    weapon: NetworkId(3),
                    recovery_ticks: 8,
                },
            )
            .unwrap();
        assert_eq!(completed.recovery_ms, Some(100.0));
        assert_eq!(cooldown(&registry, weapon), 100.0);

        let refused = ActivationToken {
            start_tick: 18,
            ..token
        };
        records.request(refused, weapon);
        let effect = records
            .outcome(
                &mut registry,
                O::InitiationRejected {
                    token: WireActivationToken {
                        start_tick: 18,
                        lane: 0,
                    },
                    recovery_ticks: 8,
                },
            )
            .unwrap();
        let host_ms = 8.0 * (1000.0 / 60.0);
        assert_eq!(effect.recovery_ms, Some(host_ms));
        assert_eq!(cooldown(&registry, weapon), host_ms);
    }
    #[test]
    fn client_weapon_active_history_does_not_expire_during_max_charge_and_waits() {
        let mut records = ActivationRecords::default();
        let token = ActivationToken {
            start_tick: 1,
            lane: postretro_foundation::ActivationLane::Primary,
        };
        records.request(token, EntityId::from_raw(1));
        records.advance_time(179000.0);
        assert!(records.records.contains_key(&token));
        records.local_terminal(token);
        records.advance_time(121000.0);
        assert!(records.records.contains_key(&token));
        records.advance_time(2000.0);
        assert!(!records.records.contains_key(&token));
    }
}
