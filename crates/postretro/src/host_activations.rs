// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
//! Host activation admission and reliable settlement around the shared machine.

use crate::{App, netcode, sim};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    ACTIVATION_TICKS_PER_SECOND, ActivationId, ActivationToken, activation_duration_ticks,
};
use postretro_net::transport::NetServer;
use postretro_net::wire;

#[cfg(test)]
mod conditioned_tests;

fn wire_token(token: ActivationToken) -> wire::WireActivationToken {
    wire::WireActivationToken {
        start_tick: token.start_tick,
        lane: token.lane as u8,
    }
}

fn recovery_ticks(registry: &EntityRegistry, weapon: EntityId) -> u32 {
    registry
        .get_component::<WeaponComponent>(weapon)
        .map_or(0, |component| {
            activation_duration_ticks(component.cooldown_remaining_ms)
        })
}

fn binding_is_live(
    registry: &EntityRegistry,
    pawn: EntityId,
    id: ActivationId,
    weapon: EntityId,
) -> bool {
    registry.exists(pawn)
        && !registry
            .get_component::<HealthComponent>(pawn)
            .is_ok_and(|health| health.current <= 0.0 || !health.current.is_finite())
        && !postretro_sim::scripting_systems::health::is_terminally_committed_to_removal(
            registry, pawn,
        )
        && registry
            .get_component::<Inventory>(pawn)
            .is_ok_and(|inventory| inventory.wieldables.contains(&Some(weapon)))
        && registry
            .get_component::<WeaponComponent>(weapon)
            .is_ok_and(|component| {
                component
                    .state
                    .activation_cursor()
                    .is_some_and(|cursor| cursor.token == id.token && cursor.pawn == id.pawn)
            })
}

/// Run before the next command, so a replaced/dropped/dead execution cannot
/// prevent a later initiation or retain unissued authorized ordinals.
pub(super) fn observe_lifecycle(
    registry: &mut EntityRegistry,
    allocator: &mut netcode::NetworkIdAllocator,
    queues: &mut netcode::HostCommandQueues,
    server: &mut NetServer,
    client: u64,
    pawn: EntityId,
    tick: u32,
) {
    let Some((id, weapon)) = queues.activations.live_binding(client) else {
        return;
    };
    if binding_is_live(registry, pawn, id, weapon) {
        return;
    }
    let recovery_ticks = recovery_ticks(registry, weapon);
    if let Ok(ComponentValue::Weapon(component)) =
        registry.get_component_value_mut(weapon, ComponentKind::Weapon)
        && component
            .state
            .activation_cursor()
            .is_some_and(|cursor| cursor.token == id.token)
    {
        component.cancel_activation();
    }
    queues.activations.terminal(client, id, tick);
    queues.activation_terminal(client, id.token);
    netcode::send_activation_outcome(
        server,
        client,
        wire::ActivationOutcome::Cancelled {
            token: wire_token(id.token),
            weapon: allocator.stamp(weapon),
            recovery_ticks,
        },
    );
}

/// Replay, concurrency, and cadence admission precede the authoritative resource
/// debit. The ledger is populated only by an actual successful machine start.
pub(super) fn guard_initiation(
    registry: &mut EntityRegistry,
    allocator: &mut netcode::NetworkIdAllocator,
    queues: &mut netcode::HostCommandQueues,
    server: &mut NetServer,
    command: &mut sim::RemotePawnCommand,
) {
    if let Some((_, bound_weapon)) = queues.activations.live_binding(command.owner_client_id) {
        command.weapon = Some(bound_weapon);
    }
    let request = command.command.activation.initiation;
    let Some(token) = command.rejected_activation.or(request) else {
        return;
    };
    let id = command.shot_id.map(|shot| ActivationId {
        pawn: shot.pawn,
        token,
    });
    let weapon_available = command.weapon.is_some_and(|weapon| {
        registry.exists(command.pawn) && registry.get_component::<WeaponComponent>(weapon).is_ok()
    });
    let ledger_allows = id.is_some_and(|id| {
        queues
            .activations
            .can_accept(command.owner_client_id, id, command.fire_tick)
    });
    let cadence = match command.weapon {
        Some(weapon) if command.rejected_activation.is_none() => queues.activation_cadence(
            command.owner_client_id,
            weapon,
            command.command.firing_slot,
            token.start_tick,
        ),
        _ => netcode::CadenceVerdict::Unrecorded,
    };
    if command.rejected_activation.is_some()
        || !command.real_command
        || !weapon_available
        || !ledger_allows
        || cadence == netcode::CadenceVerdict::Refused
    {
        command.command.activation.initiation = None;
        command.rejected_activation = Some(token);
        // A missing machine cannot emit progress, so settle its denial here.
        if !weapon_available || id.is_none() {
            reject_initiation(
                queues,
                server,
                command.owner_client_id,
                id,
                token,
                command.fire_tick,
                command
                    .weapon
                    .map_or(0, |weapon| recovery_ticks(registry, weapon)),
            );
            command.rejected_activation = None;
        }
    } else if cadence == netcode::CadenceVerdict::Eligible
        && let Some(weapon) = command.weapon
        && let Ok(ComponentValue::Weapon(component)) =
            registry.get_component_value_mut(weapon, ComponentKind::Weapon)
    {
        // Client spacing already covers the recovery this weapon's own execution
        // began. The host countdown started when the host fired, so it would
        // refuse a start that playout delivered compressed.
        component.cooldown_remaining_ms = 0.0;
    }
    // The start's shot fires along the aim its own command declared, not the
    // aim of whichever later command delivered it.
    command.start_aim = command
        .command
        .activation
        .initiation
        .and_then(|token| queues.start_aim(command.owner_client_id, token));
    // Ensure private weapon instances have a stable owner-outcome identity
    // before any shot is debited or presentation is sent.
    if let Some(weapon) = command.weapon {
        allocator.stamp(weapon);
    }
}

/// After the simulation tick, a weapon that left its client's inventory loses
/// that client's cadence record. Its own cooldown keeps whatever recovery the
/// record's credit still owes, so no later holder, that client included, fires
/// ahead of one recovery after its last credited execution. Cooldown only
/// counts down while a holder ticks the weapon, so the charge never shortens.
pub(super) fn release_departed_weapons(
    registry: &mut EntityRegistry,
    queues: &mut netcode::HostCommandQueues,
    owners: &netcode::MovementOwners,
) {
    for (weapon, owed_ticks) in queues.release_departed_cadence(registry, owners) {
        if let Ok(ComponentValue::Weapon(component)) =
            registry.get_component_value_mut(weapon, ComponentKind::Weapon)
        {
            let owed_ms = owed_ticks as f32 * 1000.0 / ACTIVATION_TICKS_PER_SECOND as f32;
            component.cooldown_remaining_ms = component.cooldown_remaining_ms.max(owed_ms);
        }
    }
}

fn reject_initiation(
    queues: &mut netcode::HostCommandQueues,
    server: &mut NetServer,
    client: u64,
    id: Option<ActivationId>,
    token: ActivationToken,
    tick: u32,
    recovery_ticks: u32,
) {
    if let Some(id) = id {
        queues.activations.terminal(client, id, tick);
    }
    queues.activation_terminal(client, token);
    netcode::send_activation_outcome(
        server,
        client,
        wire::ActivationOutcome::InitiationRejected {
            token: wire_token(token),
            recovery_ticks,
        },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostActivationFact {
    Outcome(wire::ActivationOutcome),
    ShotVerdict {
        shot_id: postretro_foundation::ShotId,
        accept: bool,
        hit_accepted: bool,
    },
}

/// Consume the shared machine's immutable decision, without reading a weapon
/// component that may have been replaced since the simulation stage.
fn record_activation_progress(
    allocator: &mut netcode::NetworkIdAllocator,
    queues: &mut netcode::HostCommandQueues,
    progress: &sim::RemoteActivationProgress,
    mut publish: impl FnMut(HostActivationFact),
) {
    let client = progress.owner_client_id;
    let Some(pawn) = allocator
        .network_id_for_entity(progress.pawn)
        .map(|id| id.0)
    else {
        return;
    };
    let advance = &progress.advance;
    let weapon = allocator.stamp(progress.weapon);
    let recovery_ticks = activation_duration_ticks(progress.recovery_ms);
    if let Some(token) = advance.initiated {
        let accepted = advance.program.as_ref().is_some_and(|program| {
            queues.activations.accept(
                client,
                ActivationId { pawn, token },
                progress.weapon,
                &program.timing,
                progress.tick,
            )
        });
        debug_assert!(
            accepted,
            "guarded successful machine initiation must admit its ledger binding"
        );
        if accepted {
            publish(HostActivationFact::Outcome(
                wire::ActivationOutcome::InitiationAccepted {
                    token: wire_token(token),
                    weapon,
                },
            ));
        }
    }
    if let Some(token) = advance.rejected {
        queues
            .activations
            .terminal(client, ActivationId { pawn, token }, progress.tick);
        queues.activation_terminal(client, token);
        publish(HostActivationFact::Outcome(
            wire::ActivationOutcome::InitiationRejected {
                token: wire_token(token),
                recovery_ticks,
            },
        ));
    }
    if let Some(charge) = advance.execution_charge {
        let token = advance
            .initiated
            .or_else(|| advance.attempted.map(|shot| shot.activation().token))
            .or_else(|| {
                queues
                    .activations
                    .live_binding(client)
                    .map(|(id, _)| id.token)
            });
        if let Some(token) = token {
            if let Some(program) = advance.program.as_ref() {
                queues
                    .activations
                    .execution(client, token, &program.timing, progress.tick);
            }
            publish(HostActivationFact::Outcome(
                wire::ActivationOutcome::ExecutionAccepted {
                    token: wire_token(token),
                    weapon,
                    charge_millionths: (charge.clamp(0.0, 1.0) * 1_000_000.0).round() as u32,
                    recovery_ticks,
                },
            ));
        }
    }
    if let Some(shot) = advance.attempted
        && advance
            .authorization
            .is_some_and(|verdict| verdict != crate::weapon::WeaponFireAuthorization::Rejected)
    {
        // The machine restarts recovery for every shot it does not reject.
        queues.activation_recovery_began(
            client,
            shot.activation().token,
            progress.weapon,
            recovery_ticks,
            advance
                .program
                .as_ref()
                .is_some_and(|program| program.timing.charge.is_some()),
        );
    }
    if let Some(shot) = advance.attempted {
        let authorized = advance.shot.is_some();
        queues.activations.settle_shot(client, shot, authorized);
        if !authorized {
            publish(HostActivationFact::ShotVerdict {
                shot_id: shot,
                accept: false,
                hit_accepted: false,
            });
        }
    }
    if let Some((token, reason)) = advance.terminal {
        queues
            .activations
            .terminal(client, ActivationId { pawn, token }, progress.tick);
        queues.activation_terminal(client, token);
        let outcome =
            if reason == postretro_combat_model::activation::ActivationTermination::Completed {
                wire::ActivationOutcome::Completed {
                    token: wire_token(token),
                    weapon,
                    recovery_ticks,
                }
            } else {
                wire::ActivationOutcome::Cancelled {
                    token: wire_token(token),
                    weapon,
                    recovery_ticks,
                }
            };
        publish(HostActivationFact::Outcome(outcome));
    }
}

impl App {
    pub(crate) fn host_record_activation_progress(
        &mut self,
        progress: &[sim::RemoteActivationProgress],
    ) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let registry = session.scripting.script_ctx.registry.clone();
        let Some(netcode::NetEndpoint::Host {
            allocator,
            command_queues,
            server,
            owners,
            ..
        }) = session.net_endpoint.as_mut()
        else {
            return;
        };
        for progress in progress {
            record_activation_progress(allocator, command_queues, progress, |fact| match fact {
                HostActivationFact::Outcome(outcome) => {
                    netcode::send_activation_outcome(server, progress.owner_client_id, outcome)
                }
                HostActivationFact::ShotVerdict {
                    shot_id,
                    accept,
                    hit_accepted,
                } => netcode::send_shot_verdict(
                    server,
                    progress.owner_client_id,
                    shot_id,
                    accept,
                    hit_accepted,
                ),
            });
        }
        release_departed_weapons(&mut registry.borrow_mut(), command_queues, owners);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapon::execution::{
        ActivationCommand, WeaponActivationAdvance, advance_weapon_activation,
    };
    use netcode::activation_ledger::OrdinalStatus;
    use postretro_entities::Transform;
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_foundation::{
        ActivationCursor, ActivationInput, ActivationLane, ActivationPhase, ActivationRelease,
        ShotId, WeaponDescriptor,
    };
    use serde_json::{Value, json};

    struct ExecutionFixture {
        registry: EntityRegistry,
        allocator: netcode::NetworkIdAllocator,
        queues: netcode::HostCommandQueues,
        pawn: EntityId,
        weapon: EntityId,
        token: ActivationToken,
        facts: Vec<HostActivationFact>,
        attempts: Vec<ShotId>,
    }

    fn action(shots: usize) -> Value {
        let mut steps = Vec::new();
        for ordinal in 0..shots {
            if ordinal > 0 {
                steps.push(json!({ "kind": "wait", "durationMs": 16 }));
            }
            steps.push(json!({ "kind": "shot" }));
        }
        json!({ "trigger": "press", "recoveryMs": 300, "steps": steps })
    }

    fn ammo(magazine: u32) -> Value {
        json!({ "kind": "ammo", "type": "rounds", "magazine": magazine, "reserve": 0 })
    }

    impl ExecutionFixture {
        fn new(secondary: Value, resource: Value) -> Self {
            let descriptor: WeaponDescriptor = serde_json::from_value(json!({
                "damage": 10, "range": 100, "resolution": "hitscan",
                "primary": action(1), "secondary": secondary, "resource": resource,
            }))
            .unwrap();
            let mut registry = EntityRegistry::new();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(
                    weapon,
                    WeaponComponent::from_descriptor_with_canonical(
                        &descriptor.validate().unwrap(),
                        Some("host-outcomes"),
                    ),
                )
                .unwrap();
            let mut inventory = Inventory::default();
            inventory.wieldables[0] = Some(weapon);
            registry.set_component(pawn, inventory).unwrap();
            let mut allocator = netcode::NetworkIdAllocator::for_test();
            allocator.stamp(pawn);
            Self {
                registry,
                allocator,
                queues: Default::default(),
                pawn,
                weapon,
                token: ActivationToken {
                    start_tick: 10,
                    lane: ActivationLane::Secondary,
                },
                facts: Vec::new(),
                attempts: Vec::new(),
            }
        }

        fn start_input(&self) -> ActivationInput {
            ActivationInput {
                initiation: Some(self.token),
                ..Default::default()
            }
        }

        fn advance_only(
            &mut self,
            tick: u32,
            input: ActivationInput,
        ) -> sim::RemoteActivationProgress {
            let pawn = self.allocator.network_id_for_entity(self.pawn).unwrap().0;
            let mut component = self
                .registry
                .get_component::<WeaponComponent>(self.weapon)
                .unwrap()
                .clone();
            let advance = advance_weapon_activation(
                &mut component,
                ActivationCommand {
                    tick,
                    pawn,
                    real_command: true,
                    input,
                    controller_starts: false,
                    primary: crate::weapon::FireButtonState {
                        pressed: false,
                        active: false,
                    },
                    secondary: crate::weapon::FireButtonState {
                        pressed: false,
                        active: false,
                    },
                },
                16.0,
                false,
                true,
            );
            let recovery_ms = component.cooldown_remaining_ms;
            self.registry.set_component(self.weapon, component).unwrap();
            sim::RemoteActivationProgress {
                pawn: self.pawn,
                owner_client_id: 7,
                weapon: self.weapon,
                tick,
                recovery_ms,
                advance,
            }
        }

        fn record(&mut self, progress: &sim::RemoteActivationProgress) {
            if let Some(shot) = progress.advance.attempted {
                self.attempts.push(shot);
            }
            record_activation_progress(&mut self.allocator, &mut self.queues, progress, |fact| {
                self.facts.push(fact)
            });
        }

        fn tick(&mut self, tick: u32, input: ActivationInput) -> WeaponActivationAdvance {
            let progress = self.advance_only(tick, input);
            self.record(&progress);
            progress.advance
        }

        fn shot(&self, ordinal: u8) -> ShotId {
            ShotId::from_parts(
                self.allocator.network_id_for_entity(self.pawn).unwrap().0,
                self.token.start_tick,
                self.token.lane,
                ordinal,
            )
        }

        fn wire_weapon(&self) -> wire::NetworkId {
            self.allocator.network_id_for_entity(self.weapon).unwrap()
        }
    }

    #[test]
    fn host_outcomes_charge_start_precedes_execution_with_bound_identity_and_resolved_charge() {
        let mut charged = action(1);
        charged["recoveryMs"] = json!(400);
        charged["charge"] = json!({ "minMs": 200, "fullMs": 1000 });
        charged["steps"][0]["scale"] = json!({ "damage": { "op": "input", "name": "charge" } });
        let mut fixture = ExecutionFixture::new(charged, ammo(4));
        fixture.tick(100, fixture.start_input());
        assert_eq!(
            fixture.facts,
            vec![HostActivationFact::Outcome(
                wire::ActivationOutcome::InitiationAccepted {
                    token: wire_token(fixture.token),
                    weapon: fixture.wire_weapon(),
                }
            )]
        );
        assert_eq!(
            fixture.queues.activations.live_binding(7),
            Some((fixture.shot(0).activation(), fixture.weapon))
        );
        for tick in 101..130 {
            fixture.tick(tick, ActivationInput::default());
        }
        let released = fixture.tick(
            130,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: fixture.token,
                    release_tick: 40,
                }),
                ..Default::default()
            },
        );
        assert!((released.shot.as_ref().unwrap().values.damage - 5.0).abs() < 1.0e-6);
        assert_eq!(
            fixture.facts[1],
            HostActivationFact::Outcome(wire::ActivationOutcome::ExecutionAccepted {
                token: wire_token(fixture.token),
                weapon: fixture.wire_weapon(),
                charge_millionths: 500_000,
                recovery_ticks: 24,
            })
        );
        assert_eq!(
            fixture.facts[2],
            HostActivationFact::Outcome(wire::ActivationOutcome::Completed {
                token: wire_token(fixture.token),
                weapon: fixture.wire_weapon(),
                recovery_ticks: 24,
            })
        );
        assert_eq!(fixture.facts.len(), 3);
        assert_eq!(fixture.attempts, vec![fixture.shot(0)]);
        assert_eq!(
            fixture.queues.activations.status(7, fixture.shot(0), 130),
            OrdinalStatus::Authorized
        );
        assert!(fixture.queues.activations.live_binding(7).is_none());
        assert_eq!(
            fixture
                .registry
                .get_component::<WeaponComponent>(fixture.weapon)
                .unwrap()
                .magazine,
            3
        );
    }

    #[test]
    fn host_outcomes_burst_settles_each_ordinal_once_and_retains_resource_refusal() {
        for magazine in [3, 2] {
            let mut fixture = ExecutionFixture::new(action(3), ammo(magazine));
            fixture.tick(100, fixture.start_input());
            assert!(matches!(
                fixture.facts[0],
                HostActivationFact::Outcome(wire::ActivationOutcome::InitiationAccepted { .. })
            ));
            assert!(matches!(
                fixture.facts[1],
                HostActivationFact::Outcome(wire::ActivationOutcome::ExecutionAccepted { .. })
            ));
            fixture.tick(101, ActivationInput::default());
            let facts_before_duplicate = fixture.facts.len();
            assert!(
                fixture
                    .tick(101, ActivationInput::default())
                    .attempted
                    .is_none()
            );
            assert_eq!(fixture.facts.len(), facts_before_duplicate);
            fixture.tick(102, ActivationInput::default());
            assert_eq!(
                fixture.attempts,
                vec![fixture.shot(0), fixture.shot(1), fixture.shot(2)]
            );
            assert!(fixture.queues.activations.live_binding(7).is_none());
            for ordinal in 0..3 {
                let status = fixture
                    .queues
                    .activations
                    .status(7, fixture.shot(ordinal), 102);
                assert_eq!(
                    status,
                    if ordinal == 2 && magazine == 2 {
                        OrdinalStatus::Rejected
                    } else {
                        OrdinalStatus::Authorized
                    }
                );
            }
            let refusal_facts = fixture
                .facts
                .iter()
                .filter(|fact| matches!(fact, HostActivationFact::ShotVerdict { .. }))
                .copied()
                .collect::<Vec<_>>();
            if magazine == 2 {
                assert_eq!(
                    refusal_facts,
                    vec![HostActivationFact::ShotVerdict {
                        shot_id: fixture.shot(2),
                        accept: false,
                        hit_accepted: false
                    }]
                );
                assert_eq!(
                    fixture.facts.last(),
                    Some(&HostActivationFact::Outcome(
                        wire::ActivationOutcome::Cancelled {
                            token: wire_token(fixture.token),
                            weapon: fixture.wire_weapon(),
                            recovery_ticks: 18,
                        }
                    ))
                );
                // A later declaration cannot promote an already refused attempt.
                fixture
                    .queues
                    .activations
                    .settle_shot(7, fixture.shot(2), true);
                assert_eq!(
                    fixture.queues.activations.status(7, fixture.shot(2), 102),
                    OrdinalStatus::Rejected
                );
            } else {
                assert!(refusal_facts.is_empty());
                assert_eq!(
                    fixture.facts.last(),
                    Some(&HostActivationFact::Outcome(
                        wire::ActivationOutcome::Completed {
                            token: wire_token(fixture.token),
                            weapon: fixture.wire_weapon(),
                            recovery_ticks: 18,
                        }
                    ))
                );
            }
            let component = fixture
                .registry
                .get_component::<WeaponComponent>(fixture.weapon)
                .unwrap();
            assert_eq!(component.magazine, 0);
            assert_eq!(component.state, WieldableState::Idle);
            assert!((component.cooldown_remaining_ms - 300.0).abs() < 1.0e-6);
        }
    }

    #[test]
    fn host_outcomes_scale_refusal_preserves_the_denied_attempt_without_spending() {
        let mut scaled = action(1);
        scaled["steps"][0]["scale"] = json!({ "damage": 2 });
        let mut fixture = ExecutionFixture::new(scaled, ammo(4));
        let mut component = fixture
            .registry
            .get_component::<WeaponComponent>(fixture.weapon)
            .unwrap()
            .clone();
        component.damage = f32::MAX;
        fixture
            .registry
            .set_component(fixture.weapon, component)
            .unwrap();
        let refused = fixture.tick(100, fixture.start_input());
        assert_eq!(
            refused.authorization,
            Some(crate::weapon::WeaponFireAuthorization::Rejected)
        );
        assert_eq!(
            fixture.facts[2],
            HostActivationFact::ShotVerdict {
                shot_id: fixture.shot(0),
                accept: false,
                hit_accepted: false
            }
        );
        assert!(matches!(
            fixture.facts.last(),
            Some(HostActivationFact::Outcome(
                wire::ActivationOutcome::Cancelled { .. }
            ))
        ));
        assert_eq!(fixture.facts.len(), 4);
        assert_eq!(
            fixture.queues.activations.status(7, fixture.shot(0), 100),
            OrdinalStatus::Rejected
        );
        assert!(fixture.queues.activations.live_binding(7).is_none());
        let component = fixture
            .registry
            .get_component::<WeaponComponent>(fixture.weapon)
            .unwrap();
        assert_eq!(component.magazine, 4);
        assert!(component.cooldown_remaining_ms.abs() < 1.0e-6);
    }

    #[test]
    fn host_outcomes_explicit_cancel_retains_prior_shot_and_rejects_unissued_ordinals() {
        let mut fixture = ExecutionFixture::new(action(3), ammo(4));
        fixture.tick(100, fixture.start_input());
        fixture.tick(
            101,
            ActivationInput {
                cancel: Some(fixture.token),
                ..Default::default()
            },
        );
        assert_eq!(
            fixture.facts.last(),
            Some(&HostActivationFact::Outcome(
                wire::ActivationOutcome::Cancelled {
                    token: wire_token(fixture.token),
                    weapon: fixture.wire_weapon(),
                    recovery_ticks: 18,
                }
            ))
        );
        assert_eq!(
            fixture.queues.activations.status(7, fixture.shot(0), 101),
            OrdinalStatus::Authorized
        );
        for ordinal in 1..3 {
            assert_eq!(
                fixture
                    .queues
                    .activations
                    .status(7, fixture.shot(ordinal), 101),
                OrdinalStatus::Rejected
            );
        }
        assert!(
            fixture
                .tick(102, ActivationInput::default())
                .attempted
                .is_none()
        );
        assert_eq!(fixture.attempts, vec![fixture.shot(0)]);
        assert!(fixture.queues.activations.live_binding(7).is_none());
        assert_eq!(
            fixture
                .registry
                .get_component::<WeaponComponent>(fixture.weapon)
                .unwrap()
                .magazine,
            3
        );
    }

    #[test]
    fn host_outcomes_install_the_admitted_program_snapshot_after_descriptor_replacement() {
        let mut charged = action(1);
        charged["charge"] = json!({ "minMs": 200, "fullMs": 1000 });
        let mut fixture = ExecutionFixture::new(charged, ammo(4));
        let progress = fixture.advance_only(100, fixture.start_input());
        let mut component = fixture
            .registry
            .get_component::<WeaponComponent>(fixture.weapon)
            .unwrap()
            .clone();
        let replacement: WeaponDescriptor = serde_json::from_value(json!({
            "damage": 25, "range": 100, "resolution": "hitscan", "primary": action(3),
        }))
        .unwrap();
        component.refresh_from_descriptor(&replacement);
        fixture
            .registry
            .set_component(fixture.weapon, component)
            .unwrap();
        fixture.record(&progress);
        assert_eq!(
            fixture.queues.activations.status(7, fixture.shot(0), 100),
            OrdinalStatus::Pending { deadline: 3880 }
        );
        assert_eq!(
            fixture.queues.activations.status(7, fixture.shot(1), 100),
            OrdinalStatus::Rejected
        );
        assert_eq!(
            fixture.facts,
            vec![HostActivationFact::Outcome(
                wire::ActivationOutcome::InitiationAccepted {
                    token: wire_token(fixture.token),
                    weapon: fixture.wire_weapon(),
                }
            )]
        );
    }

    fn fixture() -> (
        EntityRegistry,
        EntityId,
        EntityId,
        ActivationId,
        WeaponDescriptor,
    ) {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10, "range": 100, "resolution": "hitscan",
            "primary": { "trigger": "press", "recoveryMs": 300, "steps": [{ "kind": "shot" }] },
            "resource": { "kind": "ammo", "type": "rounds", "magazine": 10, "reserve": 0 },
        }))
        .unwrap();
        let id = ActivationId {
            pawn: 4,
            token: ActivationToken {
                start_tick: 12,
                lane: ActivationLane::Primary,
            },
        };
        let mut component =
            WeaponComponent::from_descriptor_with_canonical(&descriptor, Some("ion"));
        component.state = WieldableState::Executing(ActivationCursor {
            token: id.token,
            pawn: id.pawn,
            accepted_tick: 12,
            last_real_tick: 13,
            last_advanced_tick: Some(13),
            phase: ActivationPhase::Executing,
            step: 0,
            ordinal: 0,
            due_tick: 20,
            charge: 1.0,
        });
        component.magazine = 4;
        component.cooldown_remaining_ms = 87.0;
        registry.set_component(weapon, component).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        (registry, pawn, weapon, id, descriptor)
    }

    #[test]
    fn lifecycle_binding_retains_the_owned_instance_through_slot_lag_and_detects_replacement() {
        let (mut registry, pawn, weapon, id, mut descriptor) = fixture();
        assert!(binding_is_live(&registry, pawn, id, weapon));
        let other = registry.spawn(Transform::default());
        let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
        inventory.wieldables[1] = Some(other);
        inventory.active_slot = 1;
        registry.set_component(pawn, inventory).unwrap();
        assert!(
            binding_is_live(&registry, pawn, id, weapon),
            "firing-slot ownership can lead the host's active slot"
        );
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        descriptor.damage = 25.0;
        component.refresh_from_descriptor(&descriptor);
        registry.set_component(weapon, component).unwrap();
        assert!(!binding_is_live(&registry, pawn, id, weapon));
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.magazine, 4);
        assert!((component.cooldown_remaining_ms - 87.0).abs() < 1.0e-6);
    }

    #[test]
    fn lifecycle_binding_rejects_depleted_disowned_missing_and_different_executions() {
        let (mut registry, pawn, weapon, id, _) = fixture();
        let health = HealthComponent::from_descriptor(&postretro_foundation::HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        registry.set_component(pawn, health.clone()).unwrap();
        assert!(binding_is_live(&registry, pawn, id, weapon));
        let mut dead = health.clone();
        dead.current = 0.0;
        registry.set_component(pawn, dead).unwrap();
        assert!(!binding_is_live(&registry, pawn, id, weapon));
        registry.set_component(pawn, health).unwrap();
        let mut later = id;
        later.token.start_tick += 1;
        assert!(!binding_is_live(&registry, pawn, later, weapon));
        registry.set_component(pawn, Inventory::default()).unwrap();
        assert!(!binding_is_live(&registry, pawn, id, weapon));
        registry.despawn(weapon).unwrap();
        assert!(!binding_is_live(&registry, pawn, id, weapon));
    }
}
