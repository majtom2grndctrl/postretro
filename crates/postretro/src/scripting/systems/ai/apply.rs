pub(super) fn should_switch_animation(state_changed: bool, moving: bool, latch: bool) -> bool {
    state_changed || moving != latch
}

use super::engine_floor::SteeringIntent;
use super::facing::{FACING_TURN_RATE, slewed_yaw_toward};
use super::graph_eval::animation_for_path;
use super::{
    AiTickResult, AttackOutcome, ENEMY_ATTACK_EVENT, ENEMY_ATTACK_SOURCE_ID, LocomotionIntent,
};
use crate::agent_steering;
use crate::sim::{EnemyProjectilePresentationSpawn, spawn_projectile};
use glam::Quat;
use postretro_entities::Transform;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::components::mesh::{SwitchResult, switch_animation_state};
use postretro_foundation::DamagePayload;
use std::borrow::Cow;

/// Pass 3: mutate the registry from resolved enemy outcomes.
pub(super) fn apply_outcomes(
    registry: &mut postretro_entities::EntityRegistry,
    outcomes: Vec<super::EnemyOutcome>,
    tick_dt: f32,
    warned: &mut std::collections::HashSet<String>,
    blocked_warned: &mut std::collections::HashSet<postretro_entities::EntityId>,
    mut on_impact: impl FnMut(&mut postretro_entities::EntityRegistry),
) -> super::AiTickResult {
    let mut events: Vec<Cow<'static, str>> = Vec::new();
    let mut projectile_spawns = Vec::new();

    // Publish the complete compute-pass snapshot for every brain before any
    // outcome can run damage or callbacks. A contact attack may mutate a later
    // outcome's brain through the damage chokepoint; publishing that later
    // snapshot after the hit would restore its stale pre-hit perception state.
    for outcome in &outcomes {
        let _ = registry.set_component(outcome.id, outcome.brain.clone());
    }

    for mut outcome in outcomes {
        // Earlier outcomes can synchronously damage or terminally inactivate a
        // later actor. Keep every compute snapshot published above, but do not
        // let that actor's stale outcome produce any observable work.
        if crate::scripting_systems::health::is_quiescent(registry, outcome.id) {
            continue;
        }

        // The entered state's authored entry event. Raised before this tick's
        // action so a reaction reads the state the brain is now IN.
        if let Some(address) = outcome.on_enter.take() {
            events.push(Cow::Owned(address));
        }

        let path_state = agent_steering::path_state(registry, outcome.id);
        let locomotion_intent = if outcome.brain.aggro_armed {
            path_state
                .as_ref()
                .map(|path| LocomotionIntent::from_velocity(path.velocity))
                .unwrap_or(LocomotionIntent::STOPPED)
        } else {
            LocomotionIntent::STOPPED
        };

        // Steering: chase sets the destination to a selected combat slot when
        // one is available, otherwise to the raw target position. Fixed
        // position goals write their resolved destination directly. Clear
        // stands down; hold releases the agent on the tick it takes over and
        // leaves it untouched thereafter.
        // `set_destination`/`clear_destination` no-op when the enemy carries no
        // agent component.
        match outcome.steering {
            SteeringIntent::Chase => {
                if let Some(target) = outcome.target {
                    let destination = outcome.combat_slot.unwrap_or(target.position);
                    agent_steering::set_destination(registry, outcome.id, destination);
                    // Diagnostic read of the steering surface: an agent that
                    // cannot route to the destination it was given AND holds no
                    // previous path to keep following is `blocked`. Surface it
                    // once per enemy so a genuinely unroutable target (a
                    // disconnected region, or a spawn far off the navmesh —
                    // near-wall positions are snap-resolved by pathfinding and
                    // never latch this) is visible without per-tick spam. The
                    // steering tick holds a pathless blocked agent in place and
                    // keeps retrying under its replan cooldown; this only
                    // reports.
                    //
                    // `path_state` was snapshotted BEFORE the `set_destination`
                    // above, so the verdict is the steering tick's answer about
                    // the destination this enemy was chasing LAST tick, not the
                    // one just written. Reading it after the write would not
                    // help — `set_destination` deliberately leaves the plan
                    // intact and `agent_steering::tick` owns the replan — so the
                    // message says which tick it is describing instead.
                    if let Some(state) = path_state.as_ref() {
                        if state.blocked && blocked_warned.insert(outcome.id) {
                            log::warn!(
                                "[AI] enemy {} entered this tick blocked: as of the last \
                                 steering tick its agent had no path to the destination it \
                                 was chasing, so it is holding position. Warned once per \
                                 enemy.",
                                outcome.id
                            );
                        }
                    }
                }
            }
            SteeringIntent::MoveTo(goal) => {
                agent_steering::set_destination(registry, outcome.id, goal);
            }
            SteeringIntent::Clear => {
                agent_steering::clear_destination(registry, outcome.id);
            }
            SteeringIntent::Hold => {
                // `freeze` touches nothing PER TICK, but it cannot touch nothing
                // on the way IN. Entering a freeze state with no action verb
                // makes the brain unengaged, so `resolve_combat_slots` has just
                // surrendered its combat slot — and `set_destination` semantics
                // preserve the existing path, so leaving steering alone would
                // walk the agent into ground another enemy may claim on the very
                // next batch. Releasing the claim and continuing to walk into it
                // are mutually exclusive; the claim is what the slot solver
                // owns, so the walk is what has to stop. Clearing once on ENTRY
                // (not every tick) keeps the verb's contract intact afterwards:
                // a death animation, ragdoll, or scripted mover can drive the
                // frozen entity without this arm fighting it.
                if outcome.state_changed {
                    agent_steering::clear_destination(registry, outcome.id);
                }
            }
        }

        // Facing (yaw-only): compute chose this direction from the same
        // start-of-tick path state it used for the attack gate. Reusing it here
        // keeps the predicted post-slew heading and the transform write exactly
        // aligned. A committed hold-at-standoff aim therefore turns every tick
        // even before its firing leaf exposes an action.
        if let Some(direction) = outcome.facing_direction
            && let Ok(mut transform) = registry.get_component::<Transform>(outcome.id).cloned()
            && let Some(slewed_yaw) =
                slewed_yaw_toward(transform.rotation, direction, FACING_TURN_RATE * tick_dt)
        {
            transform.rotation = Quat::from_rotation_y(slewed_yaw);
            let _ = registry.set_component(outcome.id, transform);
        }

        // Fire: contact attacks route their configured amount through the
        // chokepoint to the SELECTED target id. Projectile attacks instead
        // materialize a host-owned flight entity; the shared projectile stage
        // resolves its later contact through that same chokepoint. In both
        // cases the selected target was confirmed live before the latch.
        if outcome.attacked {
            match outcome
                .attack
                .take()
                .expect("a successful enemy fire latch carries its resolution")
            {
                AttackOutcome::Contact { damage } => {
                    if let Some(target) = outcome.target {
                        apply_damage_with_context(
                            registry,
                            target.entity,
                            &DamagePayload { amount: damage },
                            DamageContext {
                                source_id: ENEMY_ATTACK_SOURCE_ID.to_string(),
                                attacker: Some(outcome.id),
                                weapon: None,
                                zone: None,
                                producer: DamageProducer::InTick,
                            },
                        );
                        on_impact(registry);
                    }
                }
                AttackOutcome::Projectile {
                    launch,
                    descriptor_class,
                } => {
                    // Enemies have no materialized weapon entity. The projectile
                    // impact path uses this id only as engine-internal damage
                    // context provenance, never as a weapon lookup.
                    if let Some(projectile) =
                        spawn_projectile(registry, outcome.id, outcome.id, *launch, None)
                    {
                        projectile_spawns.push(EnemyProjectilePresentationSpawn {
                            projectile,
                            descriptor_class,
                        });
                    }
                }
            }
            events.push(Cow::Borrowed(ENEMY_ATTACK_EVENT));
        }

        // Animation: on a state change or locomotion stop/resume, request the
        // selected animation name for the new graph/locomotion state. A failed
        // switch (`UnknownState`/`NotAnimated`) warns ONCE per distinct name and
        // keeps the prior animation — it never aborts the tick. The locomotion
        // latch is still persisted after failures so unresolved clips do not
        // re-request the same switch every tick.
        if should_switch_animation(
            outcome.state_changed,
            locomotion_intent.moving,
            outcome.brain.locomotion_moving,
        ) {
            let animation_name = animation_for_path(&outcome.brain, locomotion_intent.moving);
            if let Some(name) = animation_name {
                match switch_animation_state(registry, outcome.id, name) {
                    SwitchResult::Switched | SwitchResult::AlreadyInState => {}
                    SwitchResult::UnknownState | SwitchResult::NotAnimated => {
                        if warned.insert(format!("anim:{name}")) {
                            log::warn!(
                                "[AI] enemy animation state `{name}` could not be switched \
                                 (undeclared/unresolved on the mesh); keeping the prior \
                                 animation. Warned once per distinct name."
                            );
                        }
                    }
                }
            }
        }
        // Fold the locomotion latch into whatever the component NOW holds. The
        // damage chokepoint and `on_impact` ran since the publish above, and
        // either can mutate this entity's brain (`apply_update_enemy_state_to_brain`
        // writes exactly this component); writing the pre-callback snapshot back
        // would silently discard that. The latch is the only field this pass
        // still owns. A missing component means the entity did not survive the
        // callbacks, and there is nothing to update.
        if let Ok(mut brain) = registry
            .get_component::<BrainComponent>(outcome.id)
            .cloned()
        {
            brain.locomotion_moving = locomotion_intent.moving;
            let _ = registry.set_component(outcome.id, brain);
        }
    }

    AiTickResult {
        events,
        projectile_spawns,
    }
}
