pub(super) fn should_switch_animation(state_changed: bool, moving: bool, latch: bool) -> bool {
    state_changed || moving != latch
}

use super::attack_turns;
use super::clip_clock;
use super::engine_floor::SteeringIntent;
use super::facing::{FACING_TURN_RATE, slewed_yaw_toward};
use super::graph_eval::{animation_for_path, speed_for_path};
use super::launch;
use super::targeting::selected_target_alive;
use super::travel_variants;
use super::{
    AiTickResult, AttackOutcome, ENEMY_ATTACK_EVENT, ENEMY_ATTACK_SOURCE_ID, LocomotionIntent,
};
use crate::agent_steering;
use crate::ai_host::AiHost;
use crate::emission::entity_emitter;
use crate::nav::distance_xz;
use crate::sim::EnemyProjectilePresentationSpawn;
use crate::weapon_controller::AiWeaponControllers;
use glam::Quat;
use postretro_entities::components::agent::ArrivalLeash;
use postretro_entities::components::brain::{BrainComponent, TravelVariant};
use postretro_entities::components::health::{DamageContext, DamageProducer};
use postretro_entities::components::mesh::{SwitchResult, switch_animation_state};
use postretro_entities::{AiCue, AiEmission, Transform};
use postretro_foundation::DamagePayload;
use std::borrow::Cow;
use std::collections::HashSet;

fn commit_attack_fire(
    registry: &mut postretro_entities::EntityRegistry,
    actor: postretro_entities::EntityId,
    attack_name: String,
    cooldown_ms: f32,
) -> bool {
    let Ok(mut brain) = registry.get_component::<BrainComponent>(actor).cloned() else {
        return false;
    };
    brain
        .attack_cooldown_remaining_ms
        .insert(attack_name, cooldown_ms);
    brain.record_successful_attack_fire();
    registry.set_component(actor, brain).is_ok()
}

/// Prefix of the warn-once key for a leap whose clip cannot time its moment,
/// keyed by entity class and animation state.
const LEAP_UNTIMED_WARNING: &str = "leap-untimed:";

/// Prefix of the warn-once key for a leap spot with no walkable ground near
/// it, keyed by entity class.
const LEAP_SPOT_WARNING: &str = "leap-spot:";

/// The name warnings give an enemy's class: its descriptor's, else its model.
fn entity_class(
    registry: &postretro_entities::EntityRegistry,
    id: postretro_entities::EntityId,
) -> String {
    registry
        .get_component::<postretro_entities::provenance::DescriptorProvenance>(id)
        .map(|provenance| provenance.canonical_name.clone())
        .ok()
        .or_else(|| {
            registry
                .get_component::<postretro_entities::components::mesh::MeshComponent>(id)
                .ok()
                .map(|mesh| mesh.model.clone())
        })
        .unwrap_or_else(|| "<unnamed>".to_string())
}

/// Diagnostic read of the steering surface: an agent that cannot route to the
/// destination it was given AND holds no previous path to keep following is
/// `blocked`. Surface it once per enemy so a genuinely unroutable target (a
/// disconnected region, a spawn or leap spot far off the navmesh — near-wall
/// positions are snap-resolved by pathfinding and never latch this) is
/// visible without per-tick spam. The steering tick holds a pathless blocked
/// agent in place and keeps retrying under its replan cooldown; this only
/// reports. `going` names what the enemy was doing: "chasing", "leaping to".
///
/// `path_state` was snapshotted BEFORE this tick's destination write, so the
/// verdict is the steering tick's answer about the destination the enemy was
/// bound for LAST tick, not the one just written. Reading it after the write
/// would not help — a destination write deliberately leaves the plan intact
/// and `agent_steering::tick` owns the replan — so the message says which
/// tick it is describing instead.
fn warn_blocked(
    path_state: Option<&agent_steering::AgentPathState>,
    blocked_warned: &mut HashSet<postretro_entities::EntityId>,
    id: postretro_entities::EntityId,
    going: &str,
) {
    if path_state.is_some_and(|state| state.blocked) && blocked_warned.insert(id) {
        log::warn!(
            "[AI] enemy {id} entered this tick blocked: as of the last steering tick its \
             agent had no path to the destination it was {going}, so it is holding \
             position. Warned once per enemy."
        );
    }
}

/// Pass 3: mutate the registry from resolved enemy outcomes.
#[allow(clippy::too_many_arguments)] // orchestration keeps each apply-stage input explicit.
pub(super) fn apply_outcomes<H>(
    registry: &mut postretro_entities::EntityRegistry,
    outcomes: Vec<super::EnemyOutcome>,
    tick_dt: f32,
    warned: &mut std::collections::HashSet<String>,
    blocked_warned: &mut std::collections::HashSet<postretro_entities::EntityId>,
    weapons: &mut AiWeaponControllers,
    turns: &mut attack_turns::AttackTurnRuntime,
    host: &mut H,
) -> super::AiTickResult
where
    H: AiHost + ?Sized,
{
    let mut events: Vec<AiEmission> = Vec::new();
    let mut projectile_spawns = Vec::new();
    // Once an entity crosses the terminal simulation gate in this batch, a
    // synchronous impact policy may restore positive health but cannot make a
    // pre-lethal AI decision current again. The next tick computes a fresh one.
    let mut invalidated_entities = HashSet::new();

    // Publish the complete compute-pass snapshot for every brain before any
    // outcome can run damage or callbacks. A contact attack may mutate a later
    // outcome's brain through the damage chokepoint; publishing that later
    // snapshot after the hit would restore its stale pre-hit perception state.
    // Each agent's steered speed follows the same published, post-transition
    // path, so steering (after AI) moves under an activity's `speed` from the
    // tick it is entered through the tick before it is left, and never under
    // one entered and left in this pass.
    for outcome in &outcomes {
        if !registry.is_inventory_owned(outcome.id) {
            let _ = registry.set_component(outcome.id, outcome.brain.clone());
            agent_steering::set_activity_speed(
                registry,
                outcome.id,
                speed_for_path(&outcome.brain),
            );
        }
    }

    for mut outcome in outcomes {
        // Holding suspends world work without retiring private controller state.
        if registry.is_inventory_owned(outcome.id) {
            continue;
        }
        if outcome
            .target
            .is_some_and(|target| registry.is_inventory_owned(target.entity))
        {
            outcome.target = None;
            outcome.weapon_aim = None;
            outcome.facing_direction = None;
            outcome.combat_slot = None;
            if matches!(outcome.steering, SteeringIntent::Chase) || outcome.engaged_ring.is_some() {
                outcome.steering = SteeringIntent::Clear;
            }
        }
        // Earlier outcomes can synchronously damage or terminally inactivate a
        // later actor. Keep every compute snapshot published above, but do not
        // let that actor's stale outcome produce any observable work.
        if invalidated_entities.contains(&outcome.id)
            || host.is_quiescent(registry, outcome.id)
            || registry
                .get_component::<BrainComponent>(outcome.id)
                .is_err()
        {
            weapons.remove_actor(outcome.id);
            continue;
        }

        // The entered state's entry emission. Raised before this tick's
        // action so a reaction reads the state the brain is now IN.
        if let Some(entered) = outcome.entered.take() {
            events.push(AiEmission {
                sounds: None,
                action: None,
                shot_id: None,
                address: entered.on_enter.map(Cow::Owned),
                emitter: entity_emitter(registry, outcome.id),
                cue: AiCue::Entered {
                    graph: outcome.brain.graph.clone(),
                    path: entered.path,
                },
            });
        }

        let path_state = agent_steering::path_state(registry, outcome.id);
        // The stride reads steering velocity alone: knockback never picks it.
        let locomotion_intent = if outcome.brain.aggro_armed {
            path_state
                .as_ref()
                .map(|path| LocomotionIntent::from_velocity(path.steering_velocity()))
                .unwrap_or(LocomotionIntent::STOPPED)
        } else {
            LocomotionIntent::STOPPED
        };

        // Steering: chase sets the destination to a selected combat slot when
        // one is available, otherwise to the raw target position. A slot
        // destination is leashed to the target: the agent arrives only once
        // it stands no farther from the target than the slot does (never less
        // than the standoff), so the slot's arrival band cannot park it
        // outside the ring the standoff placed the slot on.
        //
        // A leap writes its spot paced to its clip moment. Other fixed
        // position goals write their resolved destination directly.
        //
        // Clear stands down; hold releases the agent on the tick it takes
        // over and leaves it untouched thereafter.
        // `set_destination`/`clear_destination` no-op when the enemy carries no
        // agent component.
        if outcome.leap_spot_off_mesh {
            let class = entity_class(registry, outcome.id);
            if warned.insert(format!("{LEAP_SPOT_WARNING}{class}")) {
                log::warn!(
                    "[AI] `{class}` leap: no walkable ground near its landing spot or on                      the line back to it, so it has no route there and stands through the                      release. Warned once per entity class."
                );
            }
        }
        match outcome.steering {
            SteeringIntent::Chase => {
                if let Some(target) = outcome.target {
                    let destination = outcome.combat_slot.unwrap_or(target.position);
                    let leash = outcome.combat_slot.map(|slot| ArrivalLeash {
                        anchor: target.position,
                        radius: outcome
                            .standoff_distance
                            .max(distance_xz(slot, target.position)),
                    });
                    agent_steering::set_destination_leashed(
                        registry,
                        outcome.id,
                        destination,
                        leash,
                    );
                    warn_blocked(path_state.as_ref(), blocked_warned, outcome.id, "chasing");
                }
            }
            SteeringIntent::MoveTo(goal) => match outcome.leap {
                Some(leap) => {
                    agent_steering::set_destination_paced(
                        registry,
                        outcome.id,
                        goal,
                        leap.arrive_in_s,
                    );
                    if leap.arrive_in_s.is_none() {
                        let class = entity_class(registry, outcome.id);
                        let state = outcome.brain.clip_clock.state.as_deref().unwrap_or("");
                        if warned.insert(format!("{LEAP_UNTIMED_WARNING}{class}:{state}")) {
                            log::warn!(
                                "[AI] `{class}` leap on animation state `{state}`: its clip \
                                 cannot time the attack's moment, so the leap cannot pace its \
                                 arrival and flies at its top speed. Warned once per entity \
                                 class and state."
                            );
                        }
                    }
                    warn_blocked(
                        path_state.as_ref(),
                        blocked_warned,
                        outcome.id,
                        "leaping to",
                    );
                }
                None => agent_steering::set_destination(registry, outcome.id, goal),
            },
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

        // Fire: revalidate the selected target after every earlier outcome in
        // this batch. Compute deliberately left cooldown/count state untouched;
        // those fields commit only with an effect that can actually fire.
        // Contact attacks route their configured amount through the
        // chokepoint to the SELECTED target id. Projectile attacks instead
        // materialize a host-owned flight entity; the shared projectile stage
        // resolves its later contact through that same chokepoint.
        let mut weapon_request = None;
        // A fresh unmarked grant holds only if its attack starts this tick.
        let granted_attack = outcome
            .fresh_unmarked_grant
            .and_then(|attack| attack_turns::attack_name(&outcome.brain.graph, attack));
        let mut grant_started = false;
        if let (Some(pending), Some(target)) = (outcome.attack.take(), outcome.target)
            && !invalidated_entities.contains(&target.entity)
            && selected_target_alive(registry, target.entity)
        {
            let attack_name = pending.attack_name.clone();
            let attack_fired = match pending.effect {
                AttackOutcome::Contact {
                    damage,
                    damage_type,
                } => {
                    let damage_type = registry.damage_type_index(damage_type.as_deref());
                    if commit_attack_fire(
                        registry,
                        outcome.id,
                        pending.attack_name,
                        pending.cooldown_ms,
                    ) {
                        host.apply_damage(
                            registry,
                            target.entity,
                            &DamagePayload {
                                amount: damage,
                                impulse: glam::Vec3::ZERO,
                                damage_type,
                            },
                            DamageContext {
                                source_id: ENEMY_ATTACK_SOURCE_ID.to_string(),
                                attacker: Some(outcome.id),
                                weapon: None,
                                zone: None,
                                producer: DamageProducer::InTick,
                                point: None,
                            },
                        );
                        // Observe the direct damage result before impact policy
                        // dispatch can synchronously recover it. Death-sweep
                        // latching remains downstream and untouched.
                        if host.is_quiescent(registry, target.entity) {
                            invalidated_entities.insert(target.entity);
                        }
                        host.on_impact(registry);
                        true
                    } else {
                        false
                    }
                }
                AttackOutcome::Projectile { request } => {
                    weapon_request = Some(request);
                    false
                }
            };
            grant_started |= attack_fired;
            if attack_fired {
                events.push(AiEmission {
                    sounds: None,
                    action: None,
                    shot_id: None,
                    address: Some(Cow::Borrowed(ENEMY_ATTACK_EVENT)),
                    emitter: entity_emitter(registry, outcome.id),
                    cue: AiCue::Attack {
                        graph: outcome.brain.graph.clone(),
                        attack: attack_name,
                    },
                });
            }
        }

        if registry.is_inventory_owned(outcome.id) {
            continue;
        }

        // Existing executions advance even after the activity's first-fire
        // latch closes. Apply rechecks the selected target before the shared
        // executor may debit or produce this tick's immutable shot. A flinch
        // over a release or recover cancels a burst in flight: it fires
        // nothing further, though a `"*"` or ancestor row, or the flinch
        // route on a reaction already played out, left the strike this tick.
        // Cancelled before this tick's request, an attack the row entered
        // still starts.
        if outcome.flinched {
            weapons.cancel_actor(outcome.id);
        }
        let weapon_aim = outcome.weapon_aim.filter(|_| {
            outcome.target.is_some_and(|target| {
                !invalidated_entities.contains(&target.entity)
                    && selected_target_alive(registry, target.entity)
                    && !host.is_quiescent(registry, target.entity)
            })
        });
        let fire = weapons.advance_actor(
            registry,
            outcome.id,
            weapon_request,
            weapon_aim,
            tick_dt.max(0.0) * 1000.0,
            host,
        );
        grant_started |= granted_attack.is_some_and(|granted| {
            fire.as_ref()
                .is_some_and(|fire| fire.attack_name == granted)
                || weapons.executing_attack(outcome.id) == Some(granted)
        });
        if let Some(fire) = fire
            && commit_attack_fire(
                registry,
                outcome.id,
                fire.attack_name.clone(),
                fire.recovery_ms,
            )
        {
            projectile_spawns.push(EnemyProjectilePresentationSpawn {
                projectile: fire.projectile,
                descriptor_class: fire.canonical_weapon,
            });
            events.push(AiEmission {
                sounds: Some(fire.sounds),
                action: Some(fire.action),
                shot_id: Some(fire.shot_id),
                address: Some(Cow::Borrowed(ENEMY_ATTACK_EVENT)),
                emitter: entity_emitter(registry, outcome.id),
                cue: AiCue::Attack {
                    graph: outcome.brain.graph.clone(),
                    attack: fire.attack_name,
                },
            });
        }

        if registry.is_inventory_owned(outcome.id) {
            continue;
        }

        // Animation: on a state change, locomotion stop/resume, an airborne
        // launch's clip starting or ending, a directional travel variant
        // changing, or a dropped enemy's fresh mesh asking for a redraw,
        // request the selected animation name for the new
        // graph/locomotion state. A failed switch
        // (`UnknownState`/`NotAnimated`) warns ONCE per distinct name and keeps
        // the prior animation — it never aborts the tick. The latches are still
        // persisted after failures so unresolved clips do not re-request the
        // same switch every tick.
        let airborne_clip = launch::airborne_clip(registry, outcome.id, &outcome.brain.launch);
        let animation_name =
            airborne_clip.or_else(|| animation_for_path(&outcome.brain, locomotion_intent.moving));
        let travel_variant = if airborne_clip.is_some() {
            TravelVariant::Base
        } else {
            travel_variants::presentable(
                registry,
                outcome.id,
                animation_name,
                outcome.travel_variant,
            )
        };
        let mut own_switch = None;
        // A redraw request stays pending only while the fresh mesh's clip
        // indices are still filling; a drawn state restarts the clock. A
        // switch since compute (an earlier slot's contact hit) flinched the
        // fresh mesh: the flinch stays on screen and the redraw is dropped, as
        // compute drops it for a switch before the AI stage. The next
        // compute's check reads that switch as foreign and cancels the moment.
        let redraw = outcome.brain.clip_clock.redraw
            && mesh_switches(registry, outcome.id) == outcome.brain.clip_clock.mesh_switches;
        // A flinch over a release or recover stays on screen until the enemy
        // leaves the activity: the brain never draws the strike over it, not
        // on a stop, a stride resuming or a travel variant changing. That
        // holds for one an earlier slot's contact hit just switched in too,
        // which the next compute reads as foreign and starts staggering on.
        // Not on an entry tick: there the entry's own switch draws over such
        // a flinch, a recover's as any other's, and the next compute reads
        // no foreign switch, so no stagger starts. That absorption is the
        // same one a clip-timed attack's entry makes.
        let flinch_shows = !outcome.state_changed
            && (outcome.brain.clip_clock.stagger.is_some()
                || (clip_clock::routes_flinches(&outcome.brain)
                    && mesh_switches(registry, outcome.id)
                        != outcome.brain.clip_clock.mesh_switches));
        let mut redraw_pending = false;
        let mut redrawn = false;
        if !flinch_shows
            && (should_switch_animation(
                outcome.state_changed,
                locomotion_intent.moving,
                outcome.brain.locomotion_moving,
            ) || airborne_clip.is_some() != outcome.brain.launch.airborne_presented
                || travel_variant != outcome.brain.presented_travel_variant
                || redraw)
        {
            // Only a variant the mesh declares is requested, so a missing
            // `X_left` plays `X` without warning.
            let variant_name = animation_name
                .filter(|_| travel_variant != TravelVariant::Base)
                .map(|name| format!("{name}{}", travel_variant.suffix()));
            if let Some(name) = variant_name.as_deref().or(animation_name) {
                let tracked = outcome.brain.clip_clock.state.as_deref() == Some(name);
                let before = mesh_switches(registry, outcome.id);
                if redraw && declared_but_unresolved(registry, outcome.id, name) {
                    // A dropped enemy's fresh mesh fills its clip indices after
                    // the tick it lands: draw once they resolve.
                    redraw_pending = true;
                } else {
                    match switch_animation_state(registry, outcome.id, name) {
                        SwitchResult::Switched => {
                            redrawn = tracked;
                            own_switch = Some(OwnSwitch {
                                before,
                                after: mesh_switches(registry, outcome.id),
                            });
                        }
                        // The fresh mesh already draws the state, from its
                        // start at the drop.
                        SwitchResult::AlreadyInState => redrawn = redraw && tracked,
                        SwitchResult::UnknownState | SwitchResult::NotAnimated => {
                            if warned.insert(format!("anim:{name}")) {
                                log::warn!(
                                    "[AI] enemy animation state `{name}` could not be \
                                     switched (undeclared/unresolved on the mesh); keeping \
                                     the prior animation. Warned once per distinct name."
                                );
                            }
                        }
                    }
                }
            }
        }
        // Fold the animation latches into whatever the component NOW holds. The
        // damage chokepoint and `on_impact` ran since the publish above, and
        // either can mutate this entity's brain (`apply_update_npc_state_to_brain`
        // writes exactly this component); writing the pre-callback snapshot back
        // would silently discard that. The animation latches are the only
        // fields this pass still owns. A missing component means the entity did
        // not survive the callbacks, and there is nothing to update.
        if let Ok(mut brain) = registry
            .get_component::<BrainComponent>(outcome.id)
            .cloned()
        {
            if let Some(attack) = outcome.fresh_unmarked_grant
                && !grant_started
            {
                turns.release_unstarted_grant(outcome.id, &mut brain.attack_turns, attack);
            }
            brain.locomotion_moving = locomotion_intent.moving;
            brain.launch.airborne_presented = airborne_clip.is_some();
            brain.presented_travel_variant = travel_variant;
            // The brain's own switch overwrites the count a foreign switch
            // left, so it reads a switch absorbed since compute here: an
            // earlier outcome's contact hit playing an impact clip flinched
            // the swing under way, though this switch drew over it. On an
            // entry tick the entry's own clip replaced it before the swing
            // began. Without an own switch, the next compute's check reads
            // any later switch this tick.
            if let Some(own) = own_switch {
                if !outcome.state_changed
                    && let (Some(seen), Some(before)) = (brain.clip_clock.mesh_switches, own.before)
                    && seen != before
                {
                    brain.clip_clock.moment_open = false;
                }
                brain.clip_clock.mesh_switches = own.after;
            }
            brain.clip_clock.redraw = redraw_pending;
            if redrawn {
                clip_clock::redrawn(&mut brain.clip_clock, outcome.id);
            }
            let _ = registry.set_component(outcome.id, brain);
        }
    }

    AiTickResult {
        events,
        projectile_spawns,
    }
}

/// The brain's own animation switch this tick, as the clip clock accounts it.
struct OwnSwitch {
    /// The mesh's switch count just before the switch.
    before: Option<u32>,
    /// The mesh's switch count the switch left.
    after: Option<u32>,
}

/// The mesh declares `state`, but its clip index has not filled yet.
fn declared_but_unresolved(
    registry: &postretro_entities::EntityRegistry,
    id: postretro_entities::EntityId,
    state: &str,
) -> bool {
    registry
        .get_component::<postretro_entities::components::mesh::MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
        .and_then(|animation| animation.states.get(state))
        .is_some_and(|declared| declared.clip_index.is_none())
}

/// The mesh's switch count, read around the brain's own switch.
fn mesh_switches(
    registry: &postretro_entities::EntityRegistry,
    id: postretro_entities::EntityId,
) -> Option<u32> {
    registry
        .get_component::<postretro_entities::components::mesh::MeshComponent>(id)
        .ok()?
        .animation
        .as_ref()
        .map(|animation| animation.switches.get())
}
