// See: context/lib/entity_model.md §7c · context/lib/networking.md
//! AI input policy around the canonical, persistent weapon component.
//!
//! Target selection remains in compute. Resource/timing transitions use sim's
//! shared executor only after apply rechecks the actor and selected target.
use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    ActivationInput, ActivationLane, ActivationPhase, ActivationRelease, ActivationToken, ShotId,
    WeaponActivationDescriptor, WeaponDescriptor,
};

use crate::ai_host::AiHost;
use crate::brain_programs::BrainPrograms;
use crate::sim::ProjectileSource;
use crate::weapon::execution::{
    ActivationCommand, WeaponActivationCheckpoint, advance_authoritative_weapon_resource,
    advance_weapon_activation,
};
use crate::weapon::{FireButtonState, ProjectileLaunch, freeze_weapon_shot};

#[derive(Debug, Clone, Copy)]
pub(crate) struct WeaponAim {
    pub origin: Vec3,
    pub direction: Vec3,
}

pub(crate) struct WeaponAttackRequest {
    pub canonical_weapon: String,
    pub attack_name: String,
}

pub(crate) struct CommittedWeaponFire {
    pub sounds: Arc<postretro_foundation::ActivationSounds>,
    pub action: Arc<WeaponActivationDescriptor>,
    pub shot_id: ShotId,
    pub projectile: EntityId,
    pub canonical_weapon: String,
    pub attack_name: String,
    pub recovery_ms: f32,
}

struct AiWeapon {
    descriptor: Arc<WeaponDescriptor>,
    component: WeaponComponent,
    attack_name: Option<String>,
}

#[derive(Default)]
struct ActorWeapons {
    weapons: HashMap<String, AiWeapon>,
    active: Option<String>,
    graph: Option<Arc<postretro_foundation::BehaviorGraphDescriptor>>,
    descriptor_generation: Option<u64>,
}

#[derive(Default)]
pub(crate) struct AiWeaponControllers {
    actors: HashMap<EntityId, ActorWeapons>,
    tick: u32,
}

impl AiWeaponControllers {
    /// Install/refresh only at the same lifecycle boundary as bound brain data.
    /// A valid replacement preserves live resources and owed recovery while
    /// cancelling old execution; missing/dead actors release all private data.
    pub(crate) fn sync(
        &mut self,
        registry: &EntityRegistry,
        programs: &BrainPrograms,
        mut is_quiescent: impl FnMut(EntityId) -> bool,
    ) {
        self.actors
            .retain(|actor, _| programs.get(*actor).is_some() && !is_quiescent(*actor));
        for (actor, value) in registry.iter_with_kind(ComponentKind::Brain) {
            let ComponentValue::Brain(brain) = value else {
                continue;
            };
            if is_quiescent(actor) {
                continue;
            }
            let Some(program) = programs.get(actor) else {
                continue;
            };
            let actor_weapons = self.actors.entry(actor).or_default();
            let (graph, generation) = program.installation();
            if actor_weapons
                .graph
                .as_ref()
                .is_some_and(|installed| Arc::ptr_eq(installed, graph))
                && actor_weapons.descriptor_generation == Some(generation)
            {
                continue;
            }
            actor_weapons.weapons.retain(|canonical, _| {
                brain.graph.attacks.keys().any(|attack| {
                    program
                        .resolved_projectile_attack(attack)
                        .is_some_and(|resolved| resolved.canonical_weapon_name() == canonical)
                })
            });
            for attack in brain.graph.attacks.keys() {
                let Some(resolved) = program.resolved_projectile_attack(attack) else {
                    continue;
                };
                let canonical = resolved.canonical_weapon_name();
                let descriptor = resolved.descriptor();
                match actor_weapons.weapons.get_mut(canonical) {
                    Some(weapon) if !Arc::ptr_eq(&weapon.descriptor, descriptor) => {
                        weapon.component.refresh_from_descriptor(descriptor);
                        weapon.descriptor = Arc::clone(descriptor);
                        weapon.attack_name = None;
                    }
                    Some(_) => {}
                    None => {
                        actor_weapons.weapons.insert(
                            canonical.to_string(),
                            AiWeapon {
                                descriptor: Arc::clone(descriptor),
                                component: WeaponComponent::from_descriptor_with_canonical(
                                    descriptor,
                                    Some(canonical),
                                ),
                                attack_name: None,
                            },
                        );
                    }
                }
            }
            if actor_weapons.active.as_ref().is_some_and(|canonical| {
                actor_weapons
                    .weapons
                    .get(canonical)
                    .is_none_or(|weapon| weapon.component.state.activation_cursor().is_none())
            }) {
                actor_weapons.active = None;
            }
            actor_weapons.graph = Some(Arc::clone(graph));
            actor_weapons.descriptor_generation = Some(generation);
        }
    }

    /// Private components are absent from the registry's resource sweep.
    /// Advance them through the same passive helper exactly once per AI tick.
    pub(crate) fn begin_tick(&mut self, dt_ms: f32) {
        self.tick = self.tick.wrapping_add(1);
        for actor in self.actors.values_mut() {
            for weapon in actor.weapons.values_mut() {
                advance_authoritative_weapon_resource(&mut weapon.component, dt_ms);
            }
        }
    }

    pub(crate) fn remove_actor(&mut self, actor: EntityId) {
        self.actors.remove(&actor);
    }

    pub(crate) fn cancel_activations(&mut self) {
        for actor in self.actors.values_mut() {
            actor.active = None;
            for weapon in actor.weapons.values_mut() {
                weapon.component.cancel_activation();
                weapon.attack_name = None;
            }
        }
    }

    /// Charging and waits commit every valid tick. Only an actual failed
    /// materialization restores the shot transaction; passive regen stays paid.
    pub(crate) fn advance_actor<H: AiHost + ?Sized>(
        &mut self,
        registry: &mut EntityRegistry,
        actor: EntityId,
        request: Option<WeaponAttackRequest>,
        aim: Option<WeaponAim>,
        dt_ms: f32,
        host: &mut H,
    ) -> Option<CommittedWeaponFire> {
        let actor_weapons = self.actors.get_mut(&actor)?;
        if aim.is_none() {
            for weapon in actor_weapons.weapons.values_mut() {
                weapon.component.cancel_activation();
                weapon.attack_name = None;
            }
            actor_weapons.active = None;
        }
        let request = request.filter(|_| aim.is_some());
        if let Some(request) = request.as_ref()
            && actor_weapons
                .active
                .as_ref()
                .is_some_and(|active| active != &request.canonical_weapon)
        {
            if let Some(previous) = actor_weapons
                .active
                .take()
                .and_then(|name| actor_weapons.weapons.get_mut(&name))
            {
                previous.component.cancel_activation();
                previous.attack_name = None;
            }
        }
        let mut committed = None;
        for (canonical, weapon) in &mut actor_weapons.weapons {
            let starts = request
                .as_ref()
                .filter(|request| &request.canonical_weapon == canonical)
                .filter(|_| weapon.component.state.activation_cursor().is_none());
            let mut input = ActivationInput::default();
            if let Some(request) = starts {
                input.initiation = Some(ActivationToken {
                    start_tick: self.tick,
                    lane: ActivationLane::Primary,
                });
                weapon.attack_name = Some(request.attack_name.clone());
            }
            // AI has no physical release channel. Its controller explicitly
            // releases at authored full charge; the shared executor retains
            // all ordinary minimum, clamp, identity and cancellation rules.
            if let Some(cursor) = weapon.component.state.activation_cursor()
                && cursor.phase == ActivationPhase::Charging
                && let Some(timing) = weapon
                    .component
                    .activation_programs
                    .primary
                    .as_ref()
                    .and_then(|program| program.timing.charge)
                && self.tick.wrapping_sub(cursor.token.start_tick) >= timing.full_ticks
            {
                input.release = Some(ActivationRelease {
                    token: cursor.token,
                    release_tick: self.tick,
                });
            }
            let checkpoint = WeaponActivationCheckpoint::capture(&weapon.component);
            let command = ActivationCommand {
                tick: self.tick,
                pawn: actor.to_raw(),
                real_command: true,
                input,
                controller_starts: false,
                primary: FireButtonState {
                    pressed: starts.is_some(),
                    active: starts.is_some() || weapon.attack_name.is_some(),
                },
                secondary: FireButtonState {
                    pressed: false,
                    active: false,
                },
            };
            let advanced = advance_weapon_activation(
                &mut weapon.component,
                command,
                dt_ms,
                false,
                aim.is_some(),
            );
            if advanced.initiated.is_some() {
                actor_weapons.active = Some(canonical.clone());
            }
            if let Some(attempt) = advanced.shot {
                let shot = freeze_weapon_shot(&weapon.component, attempt);
                let aim = aim.expect("a live AI execution retains an aim");
                let projectile = shot
                    .projectile
                    .as_ref()
                    .expect("AI sync admits projectile weapons");
                let launch = ProjectileLaunch {
                    sounds: Some(shot.sounds.clone()),
                    origin: aim.origin,
                    direction: aim.direction,
                    speed: projectile.speed,
                    radius: projectile.radius,
                    range: shot.activation.values.range,
                    lifetime: projectile.lifetime_ms / 1000.0,
                    damage: shot.activation.values.damage,
                    knockback_impulse: shot.knockback.map_or(Vec3::ZERO, |push| {
                        postretro_foundation::knockback_impulse(
                            push.speed,
                            push.upward_bias,
                            aim.direction,
                        )
                    }),
                    credit_source: shot.credit_source.clone(),
                    descriptor: projectile.clone(),
                    splash: shot.splash.clone(),
                    action: Some(Arc::clone(shot.action())),
                    shot_id: Some(shot.activation.shot_id),
                    model_scale: shot.projectile_model_scale,
                };
                let source = ProjectileSource {
                    weapon: Some(canonical.clone()),
                    activation: None,
                };
                if let Some(projectile) =
                    host.spawn_projectile(registry, actor, actor, launch, source)
                {
                    committed = Some(CommittedWeaponFire {
                        sounds: shot.sounds.clone(),
                        action: Arc::clone(shot.action()),
                        shot_id: shot.activation.shot_id,
                        projectile,
                        canonical_weapon: canonical.clone(),
                        attack_name: weapon
                            .attack_name
                            .clone()
                            .expect("AI execution retains its initiating attack"),
                        recovery_ms: shot.activation.recovery_ms,
                    });
                } else {
                    checkpoint.restore(&mut weapon.component);
                }
            }
            if weapon.component.state.activation_cursor().is_none() {
                weapon.attack_name = None;
                if actor_weapons.active.as_ref() == Some(canonical) {
                    actor_weapons.active = None;
                }
            }
        }
        committed
    }

    #[cfg(test)]
    pub(crate) fn weapon(&self, actor: EntityId, canonical: &str) -> Option<&WeaponComponent> {
        self.actors
            .get(&actor)?
            .weapons
            .get(canonical)
            .map(|weapon| &weapon.component)
    }

    #[cfg(test)]
    pub(crate) fn weapon_mut(
        &mut self,
        actor: EntityId,
        canonical: &str,
    ) -> Option<&mut WeaponComponent> {
        self.actors
            .get_mut(&actor)?
            .weapons
            .get_mut(canonical)
            .map(|weapon| &mut weapon.component)
    }
}
