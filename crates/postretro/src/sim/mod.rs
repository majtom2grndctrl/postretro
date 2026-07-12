// Headless fixed-tick game-state advance seam.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::Vec3;

use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::kinematic_mover::{self, MoverTickStateTable};
use crate::movement::MovementInput;
use crate::nav::NavGraph;
use crate::netcode::{AuthorizedShot, OpenAuthorizedShot, ShotId};
use crate::scripting_systems;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::trigger_bindings::{TriggerBindingTable, TriggerResidualHandle};
#[cfg(test)]
use crate::trigger_system::TriggerEvent;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};
use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::{BrainComponent, LogicalState};
use postretro_entities::components::health::{
    DamageContext, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::weapon::{UNKNOWN_WEAPON_CREDIT_SOURCE, WeaponComponent};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, SlotTable};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

#[derive(Debug, Clone)]
pub(crate) struct SimCommand {
    pub(crate) movement: MovementInput,
    pub(crate) fire_button: FireButtonState,
    pub(crate) reload: bool,
    /// Use rising edge routed to the host-authoritative trigger stage. Kept on
    /// the full command alongside fire/reload; `MovementInput` mirrors it for the
    /// client-prediction input boundary.
    pub(crate) use_pressed: bool,
}

pub(crate) struct PostMovementCommand {
    pub(crate) aim_origin: Vec3,
    pub(crate) aim_direction: Vec3,
}

#[derive(Debug, Clone)]
pub(crate) struct RemotePawnCommand {
    pub(crate) pawn: EntityId,
    pub(crate) owner_client_id: u64,
    pub(crate) weapon: Option<EntityId>,
    pub(crate) shot_id: Option<ShotId>,
    pub(crate) fire_tick: u32,
    #[allow(dead_code)]
    pub(crate) client_tick: u32,
    pub(crate) command: SimCommand,
}

/// Host-only inputs for the trigger stage. The system itself consumes the
/// per-player map, never an action snapshot; local and remote use edges are
/// keyed by `PlayerId` at this boundary.
pub(crate) struct TriggerTickContext<'a> {
    pub(crate) system: &'a mut TriggerSystem,
    pub(crate) bridge: &'a TriggerVolumeBridge,
    pub(crate) bindings: &'a TriggerBindingTable,
    pub(crate) slot_table: Rc<RefCell<SlotTable>>,
    pub(crate) use_edges: &'a HashMap<PlayerId, bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReloadDelivery {
    pub(crate) pawn: EntityId,
    pub(crate) weapon: EntityId,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct TickEvents {
    pub(crate) movement: Vec<&'static str>,
    pub(crate) ai: Vec<&'static str>,
    pub(crate) weapon: Vec<&'static str>,
    pub(crate) death: Vec<String>,
    pub(crate) authorized_shots: Vec<OpenAuthorizedShot>,
    pub(crate) reload_deliveries: Vec<ReloadDelivery>,
    /// Bound trigger residuals drained app-side after every fixed tick this frame.
    pub(crate) trigger_residuals: Vec<TriggerResidualHandle>,
    /// Test-only fixed-tick trace. Production consumes residual handles only;
    /// keeping the detailed sequence out of non-test builds avoids a hot-path
    /// diagnostic allocation.
    #[cfg(test)]
    pub(crate) trigger_fires: Vec<TriggerEvent>,
    #[cfg(test)]
    pub(crate) trigger_command_fires: Vec<TriggerCommandFire>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerCommandFire {
    pub(crate) event: TriggerEvent,
    pub(crate) commands: Vec<crate::trigger_bindings::BoundTriggerCommandKind>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn simulate_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    gravity: f32,
    active_wieldable: Option<EntityId>,
    anim_time: f64,
    progress_tracker: &mut ProgressTracker,
    ai_warned: &mut HashSet<String>,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_commands: &[RemotePawnCommand],
    command: &SimCommand,
    mut post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
    trigger_context: Option<TriggerTickContext<'_>>,
) -> TickEvents {
    registry.borrow_mut().snapshot_transforms();

    {
        let mut registry = registry.borrow_mut();
        kinematic_mover::run_kinematic_mover_tick(&mut registry, mover_tick_states, tick_dt);
    }

    let remote_pawn_inputs: Vec<(EntityId, MovementInput)> = remote_pawn_commands
        .iter()
        .map(|remote| (remote.pawn, remote.command.movement.clone()))
        .collect();

    let combined_collision =
        CombinedCollisionWorld::new(collision_world, mover_colliders, mover_tick_states);
    let mut movement = {
        let mut registry = registry.borrow_mut();
        host_movement::run_host_movement_tick(
            &mut registry,
            &combined_collision,
            gravity,
            &remote_pawn_inputs,
            tick_dt,
        )
    };
    movement.extend(run_movement_tick(
        &registry,
        &combined_collision,
        gravity,
        &command.movement,
        tick_dt,
    ));
    let mut trigger_residuals = Vec::new();
    #[cfg(test)]
    let mut trigger_fires = Vec::new();
    #[cfg(test)]
    let mut trigger_command_fires = Vec::new();
    if let Some(trigger_context) = trigger_context {
        let mut players: Vec<AuthoritativePlayer> = remote_pawn_commands
            .iter()
            .map(|remote| AuthoritativePlayer {
                id: PlayerId::Remote(remote.owner_client_id),
                pawn: remote.pawn,
            })
            .collect();
        let local_player = {
            let registry = registry.borrow();
            local_movement_pawn(&registry)
        };
        if let Some(pawn) = local_player {
            let id = PlayerId::Local(pawn);
            players.push(AuthoritativePlayer { id, pawn });
        }
        let mut registry = registry.borrow_mut();
        let mut slot_table = trigger_context.slot_table.borrow_mut();
        let _report = trigger_context.system.run_authoritative_tick_with_dispatch(
            &mut registry,
            trigger_context.bridge,
            &players,
            trigger_context.use_edges,
            tick_dt,
            |event, registry| {
                let execution = trigger_context.bindings.execute(
                    event.fire.trigger,
                    event.edge,
                    registry,
                    &mut slot_table,
                );
                #[cfg(test)]
                {
                    trigger_fires.push(event.clone());
                    trigger_command_fires.push(TriggerCommandFire {
                        event: event.clone(),
                        commands: execution.commands.clone(),
                    });
                }
                if let Some(handle) = execution.residual() {
                    trigger_residuals.push(handle);
                }
            },
        );
    }
    let ai = {
        let mut registry = registry.borrow_mut();
        scripting_systems::ai::run_ai_tick_with_navigation(
            &mut registry,
            ai_warned,
            tick_dt,
            nav_graph,
            Some(collision_world),
        )
    };

    let post_movement_command = post_movement(&registry);

    {
        let mut registry = registry.borrow_mut();
        // AgentTickResult only carries a diagnostic `replans` counter, not observable sim state, so the return value is intentionally discarded.
        let _ = agent_steering::tick(&mut registry, collision_world, nav_graph, gravity, tick_dt);
        update_brain_animation_playback_rates(&mut registry, anim_time);
    }

    let (authorized_shots, mut reload_deliveries) =
        run_remote_weapon_commands(&registry, remote_pawn_commands, tick_dt);
    let weapon_fire = weapon_fire_command(command.fire_button, post_movement_command);
    let weapon = run_weapon_fire_tick(
        &registry,
        active_wieldable,
        &weapon_fire,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    // Route the OWN pawn's reload through the same named seam every remote pawn uses, so
    // the ammo spec adds no separate own-pawn plumbing (Task 4, "one seam"). Appended last,
    // mirroring the own fire path above. No-op when the local pawn or its active weapon is
    // unresolvable, mirroring the remote path's weapon guard.
    let own_pawn = {
        let registry = registry.borrow();
        local_movement_pawn(&registry)
    };
    if let (Some(pawn), Some(weapon_id)) = (own_pawn, active_wieldable)
        && let Some(delivery) = deliver_reload_to_weapon(pawn, weapon_id, command.reload)
    {
        reload_deliveries.push(delivery);
    }
    let death = run_death_sweep(&registry, progress_tracker);

    TickEvents {
        movement,
        ai,
        weapon,
        death,
        authorized_shots,
        reload_deliveries,
        trigger_residuals,
        #[cfg(test)]
        trigger_fires,
        #[cfg(test)]
        trigger_command_fires,
    }
}

/// Update brain-driven walk playback after steering has resolved this tick's
/// velocity. `anim_time` is the slow-mo/freeze-gated animation clock sampled by
/// the renderer. The AI pass's locomotion intent is deliberately not reused: it
/// is a pre-steering, squared-speed read from the prior tick, while this path
/// must match the motion the steering system actually produced.
fn update_brain_animation_playback_rates(registry: &mut EntityRegistry, anim_time: f64) {
    let mut rebases = Vec::new();

    for (id, _) in registry.iter_with_kind(ComponentKind::Brain) {
        let Ok(brain) = registry.get_component::<BrainComponent>(id) else {
            continue;
        };
        let Ok(agent) = registry.get_component::<AgentComponent>(id) else {
            continue;
        };
        let Some(path_state) = agent_steering::path_state(registry, id) else {
            continue;
        };
        let speed_xz = Vec3::new(path_state.velocity.x, 0.0, path_state.velocity.z).length();
        let raw_ratio = if agent.move_speed > 0.0 {
            speed_xz / agent.move_speed
        } else {
            1.0
        };

        let Some(animation) = registry
            .get_component::<MeshComponent>(id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
        else {
            continue;
        };
        let rate_input =
            if animation.current_state == brain.tuning.states.animation_for(LogicalState::Alert) {
                raw_ratio
            } else {
                1.0
            };
        if animation.playback_rate_needs_update(rate_input) {
            rebases.push((id, rate_input));
        }
    }

    for (id, rate_input) in rebases {
        // The read-only predicate above centralizes clamping and the epsilon
        // policy, so clone/write only the components whose timeline will rebase.
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let Some(animation) = mesh.animation.as_mut() else {
            continue;
        };

        animation.update_playback_rate(rate_input, anim_time);
        let _ = registry.set_component(id, mesh);
    }
}

mod host_movement;

#[cfg(test)]
pub(crate) use host_movement::run_host_movement_tick;

#[cfg(test)]
mod determinism_tests;
#[cfg(test)]
mod divergence_spike_tests;
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) mod predict_reconcile;

/// Single-player / single-pawn movement stage. Resolves the local movement pawn via
/// the registry marker, then drives it through the shared host multi-pawn seam
/// (`host_movement::run_host_movement_tick`) with a one-element input list. The host
/// netcode path bypasses this entirely and calls the seam directly with EVERY
/// authoritative pawn (Task 4) — `local_movement_pawn` is the single-player resolver
/// only, never the authoritative-host resolver.
fn run_movement_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision: &impl crate::movement::MovementCollisionSource,
    gravity: f32,
    input: &MovementInput,
    tick_dt: f32,
) -> Vec<&'static str> {
    let local = {
        let registry = registry.borrow();
        local_movement_pawn(&registry)
    };
    let Some(id) = local else {
        return Vec::new();
    };

    let pawn_inputs = [(id, input.clone())];
    let mut registry = registry.borrow_mut();
    host_movement::run_host_movement_tick(&mut registry, collision, gravity, &pawn_inputs, tick_dt)
}

/// Resolve the local movement pawn: registry marker first, then first
/// `PlayerMovement` entity. See also `followed_player_pawn` (main.rs)
/// and `player_position` (scripting/systems/ai.rs).
fn local_movement_pawn(registry: &EntityRegistry) -> Option<EntityId> {
    if let Some(id) = registry.local_player_pawn() {
        if matches!(
            registry.has_component_kind(id, ComponentKind::PlayerMovement),
            Ok(true)
        ) {
            return Some(id);
        }
    }

    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()
        .map(|(id, _)| id)
}

fn weapon_fire_command(
    button: FireButtonState,
    post_movement: PostMovementCommand,
) -> WeaponFireCommand {
    // The aim normalization and `can_fire` gate below are degenerate-input guards.
    // `camera.aim_ray()` already returns normalized, finite values in normal operation;
    // these checks protect against NaN/zero vectors from headless or mocked callers.
    if post_movement.aim_origin.is_finite()
        && let Some(aim_direction) = normalize_aim_direction(post_movement.aim_direction)
    {
        return WeaponFireCommand {
            button,
            aim_origin: post_movement.aim_origin,
            aim_direction,
            can_fire: true,
        };
    }

    WeaponFireCommand {
        button,
        aim_origin: Vec3::ZERO,
        aim_direction: Vec3::Z,
        can_fire: false,
    }
}

fn normalize_aim_direction(direction: Vec3) -> Option<Vec3> {
    if !direction.is_finite() {
        return None;
    }
    let length_squared = direction.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return None;
    }
    Some(direction / length_squared.sqrt())
}

fn run_remote_weapon_commands(
    registry: &Rc<RefCell<EntityRegistry>>,
    remote_pawn_commands: &[RemotePawnCommand],
    tick_dt: f32,
) -> (Vec<OpenAuthorizedShot>, Vec<ReloadDelivery>) {
    let mut registry = registry.borrow_mut();
    let mut authorized = Vec::new();
    let mut reload_deliveries = Vec::new();

    for remote in remote_pawn_commands {
        let Some(weapon) = remote.weapon else {
            continue;
        };
        if let Some(delivery) = deliver_reload_to_weapon(remote.pawn, weapon, remote.command.reload)
        {
            reload_deliveries.push(delivery);
        }

        let command = WeaponFireCommand {
            button: remote.command.fire_button,
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::Z,
            // Repurposes `can_fire` (elsewhere "aim valid") to mean "pawn has a NetworkId";
            // the real fire gate is `button` -> `wants_fire`. The host casts no local aim ray.
            can_fire: remote.shot_id.is_some(),
        };
        let result = weapon::tick_state_only(&mut registry, Some(weapon), &command, tick_dt);
        if result != WeaponFireAuthorization::Accepted {
            continue;
        }
        let Some(shot_id) = remote.shot_id else {
            continue;
        };
        let Ok(weapon_component) = registry.get_component::<WeaponComponent>(weapon) else {
            continue;
        };
        let effective = weapon_component.effective();
        authorized.push(OpenAuthorizedShot {
            shot: AuthorizedShot {
                shot_id,
                pawn: remote.pawn,
                weapon,
                fire_tick: remote.fire_tick,
                damage: effective.damage,
                range: effective.range,
                pellet_count: 1,
                credit_source: effective.credit_source,
            },
            owner_client_id: remote.owner_client_id,
        });
    }

    (authorized, reload_deliveries)
}

fn deliver_reload_to_weapon(
    pawn: EntityId,
    weapon: EntityId,
    reload: bool,
) -> Option<ReloadDelivery> {
    if reload {
        Some(ReloadDelivery { pawn, weapon })
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn run_weapon_fire_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    active_wieldable: Option<EntityId>,
    command: &WeaponFireCommand,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
) -> Vec<&'static str> {
    let mut registry = registry.borrow_mut();
    let events = weapon::tick_resolved(
        &mut registry,
        active_wieldable,
        command,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    if let Some(impact) = events.impact.as_ref() {
        weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);
        let attacker = local_movement_pawn(&registry);
        apply_weapon_impact_damage(&mut registry, active_wieldable, attacker, impact);
    }
    events.event_names()
}

pub(crate) fn apply_weapon_impact_damage(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
) {
    let (Some(_), weapon::ActivationOutcome::Hit(payload)) = (impact.target, impact.outcome) else {
        return;
    };
    let Some(weapon_id) = active_wieldable else {
        log::warn!("[Weapon] hitscan impact had no active wieldable; dropping damage");
        return;
    };
    let Ok(component) = registry.get_component::<WeaponComponent>(weapon_id) else {
        log::warn!("[Weapon] active wieldable {weapon_id} has no WeaponComponent; dropping damage");
        return;
    };

    let effective = component.effective();
    apply_weapon_impact_damage_with_source(
        registry,
        weapon_id,
        attacker,
        impact,
        effective.credit_source,
        payload.amount,
    );
}

pub(crate) fn apply_authorized_weapon_impact_damage(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
    credit_source: String,
    damage_amount: f32,
) {
    apply_weapon_impact_damage_with_source(
        registry,
        weapon_id,
        attacker,
        impact,
        credit_source,
        damage_amount,
    );
}

fn apply_weapon_impact_damage_with_source(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
    credit_source: String,
    damage_amount: f32,
) {
    let (Some(target), weapon::ActivationOutcome::Hit(_)) = (impact.target, impact.outcome) else {
        return;
    };
    let source_id = if credit_source.is_empty() {
        log::warn!(
            "[Weapon] active wieldable {weapon_id} resolved an empty credit source; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
        UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
    } else {
        credit_source
    };
    let multiplier = impact
        .zone
        .as_deref()
        .and_then(|tag| {
            registry
                .get_component::<HealthComponent>(target)
                .ok()
                .and_then(|health| health.zone_multipliers.get(tag).copied())
        })
        .unwrap_or(1.0);
    let scaled = weapon::DamagePayload {
        amount: damage_amount * multiplier,
    };
    if !scaled.amount.is_finite() {
        log::warn!(
            "[Weapon] scaled damage amount {} is non-finite; dropping damage",
            scaled.amount
        );
        return;
    }
    apply_damage_with_context(
        registry,
        target,
        &scaled,
        DamageContext {
            source_id,
            attacker,
            weapon: Some(weapon_id),
            zone: impact.zone.clone(),
        },
    );
}

pub(crate) fn run_death_sweep(
    registry: &Rc<RefCell<EntityRegistry>>,
    progress_tracker: &mut ProgressTracker,
) -> Vec<String> {
    let report = {
        let mut registry = registry.borrow_mut();
        scripting_systems::health::sweep_deaths(&mut registry)
    };

    let mut events = Vec::new();
    for tags in &report.killed_tags {
        events.extend(progress_tracker.on_entity_killed(tags));
    }
    if report.player_died {
        events.push(scripting_systems::health::PLAYER_DIED_EVENT.to_string());
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionWorld;
    use crate::kinematic_mover::MoverTickStateTable;
    use crate::scripting::reactions::registry::ReactionPrimitiveRegistry;
    use crate::scripting::reactions::system_commands::{
        SystemReactionRegistry, register_system_reaction_primitives,
    };
    use crate::scripting_systems::hit_zones::HitZoneStore;
    use crate::trigger_bindings::TriggerBindingTable;
    use crate::weapon::FireButtonState;
    use glam::Vec2;
    use postretro_entities::components::mesh::{
        AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
        resolve_pending_animation_stamps,
    };
    use postretro_entities::{
        DataRegistry, KinematicMoverComponent, KinematicMoverMode, MoverCommand, NamedReaction,
        NumericRange, PrimitiveDescriptor, ReactionDescriptor, SlotOwnership, SlotRecord,
        SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation, TriggerFireMode,
        TriggerVolumeComponent,
    };
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, FireMode, GroundParams, PlayerMovementComponent,
        PlayerMovementDescriptor, ResolutionMode, SpeedParams, WeaponDescriptor,
    };
    use postretro_net::wire::NetworkId;
    use postretro_scripting_core::reaction_dispatch::fire_prepartitioned_reactions_with_sequences;
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use std::collections::{HashMap, HashSet};

    fn weapon_component(credit_source: &str) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: Some(credit_source.to_string()),
        })
    }

    fn zero_movement() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
        }
    }

    fn sim_command(fire: bool, reload: bool) -> SimCommand {
        SimCommand {
            movement: zero_movement(),
            fire_button: FireButtonState {
                pressed: fire,
                active: fire,
            },
            reload,
            use_pressed: false,
        }
    }

    fn trigger_movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        })
    }

    fn animated_mesh() -> MeshComponent {
        let mut states = HashMap::new();
        for (name, clip_index) in [("idle", 0), ("attack", 1)] {
            states.insert(
                name.to_string(),
                AnimationState {
                    clip: format!("{name}_clip"),
                    looping: name == "idle",
                    crossfade_ms: DEFAULT_CROSSFADE_MS,
                    interrupt: InterruptPolicy::Smooth,
                    clip_index: Some(clip_index),
                },
            );
        }
        MeshComponent::animated(
            "test_model".to_string(),
            MeshAnimation::new(states, "idle".into()),
        )
    }

    fn trigger_component(on_fire: &str, armed: bool) -> TriggerVolumeComponent {
        TriggerVolumeComponent::new(
            TriggerActivation::Touch,
            String::new(),
            on_fire.to_string(),
            String::new(),
            MoverCommand::Start,
            TriggerFireMode::Multiple,
            0.0,
            armed,
        )
    }

    fn trigger_slots() -> SlotTable {
        let mut slots = SlotTable::new();
        slots
            .insert_namespace(
                "trigger",
                vec![(
                    "flag".into(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: Some(NumericRange { min: 0.0, max: 1.0 }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: Default::default(),
                    }),
                )],
            )
            .unwrap();
        slots
    }

    fn remote_command(
        pawn: EntityId,
        weapon: Option<EntityId>,
        network_id: u32,
        client_tick: u32,
        fire: bool,
        reload: bool,
    ) -> RemotePawnCommand {
        RemotePawnCommand {
            pawn,
            owner_client_id: 7,
            weapon,
            shot_id: Some(ShotId::from_parts(NetworkId(network_id), client_tick)),
            fire_tick: 33,
            client_tick,
            command: sim_command(fire, reload),
        }
    }

    fn run_remote_only_tick(
        registry: Rc<RefCell<EntityRegistry>>,
        remote: &[RemotePawnCommand],
    ) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_warned = HashSet::new();
        let mover_colliders = Vec::new();
        let mut mover_states = MoverTickStateTable::default();
        simulate_tick(
            registry,
            &world,
            &hit_zones,
            None,
            -9.81,
            None,
            0.0,
            &mut progress,
            &mut ai_warned,
            &mover_colliders,
            &mut mover_states,
            remote,
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            None,
        )
    }

    #[test]
    fn trigger_consequences_run_in_tick_once_and_residual_reaches_app_drain() {
        let script_ctx = postretro_entities::ScriptCtx::new();
        *script_ctx.slot_table.borrow_mut() = trigger_slots();
        let registry = script_ctx.registry.clone();
        let (source_trigger, mover, animated_target, arm_target) = {
            let mut registry = registry.borrow_mut();
            let player = registry.spawn(Transform::default());
            registry.set_component(player, trigger_movement()).unwrap();

            let source_trigger = registry.spawn(Transform::default());
            registry
                .set_component(source_trigger, trigger_component("triggered", true))
                .unwrap();

            let mover = registry.spawn(Transform::default());
            registry.set_tags(mover, vec!["door".into()]).unwrap();
            registry
                .set_component(
                    mover,
                    KinematicMoverComponent::new(
                        1,
                        vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                        vec!["start".into(), "end".into()],
                        1.0,
                        0.0,
                        KinematicMoverMode::Once,
                        false,
                    ),
                )
                .unwrap();

            let animated_target = registry.spawn(Transform::default());
            registry
                .set_tags(animated_target, vec!["enemy".into()])
                .unwrap();
            registry
                .set_component(animated_target, animated_mesh())
                .unwrap();
            registry
                .set_component(
                    animated_target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: None,
                        death_handled: false,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            resolve_pending_animation_stamps(&mut registry, 0.0);

            let arm_target = registry.spawn(Transform::default());
            registry.set_tags(arm_target, vec!["rearm".into()]).unwrap();
            registry
                .set_component(arm_target, trigger_component("", false))
                .unwrap();

            (source_trigger, mover, animated_target, arm_target)
        };

        let mut data = DataRegistry::new();
        let primitive =
            |primitive: &str, tag: Option<&str>, args: serde_json::Value| NamedReaction {
                name: "triggered".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: primitive.into(),
                    tag: tag.map(str::to_string),
                    on_complete: None,
                    args,
                }),
            };
        data.populate_level(
            vec![
                primitive("moverStart", Some("door"), serde_json::json!({})),
                primitive(
                    "applyDamage",
                    Some("enemy"),
                    serde_json::json!({ "amount": 10 }),
                ),
                primitive(
                    "setAnimationState",
                    Some("enemy"),
                    serde_json::json!({ "state": "attack" }),
                ),
                primitive(
                    "setState",
                    None,
                    serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                ),
                primitive("armTrigger", Some("rearm"), serde_json::json!({})),
                primitive(
                    "flashScreen",
                    None,
                    serde_json::json!({ "color": [1.0, 0.0, 0.0, 1.0], "durationMs": 30.0 }),
                ),
            ],
            Vec::new(),
            &[],
        );
        let bindings =
            TriggerBindingTable::build(&registry.borrow(), &data, &script_ctx.slot_table.borrow());
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(source_trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
        let mut trigger_system = TriggerSystem::default();
        let mut progress = ProgressTracker::new();
        let mut ai_warned = HashSet::new();
        let mut mover_states = MoverTickStateTable::default();
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let use_edges = HashMap::new();

        let events = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            None,
            0.0,
            &mut progress,
            &mut ai_warned,
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            Some(TriggerTickContext {
                system: &mut trigger_system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: script_ctx.slot_table.clone(),
                use_edges: &use_edges,
            }),
        );

        assert_eq!(events.trigger_residuals.len(), 1);
        assert_eq!(events.trigger_command_fires.len(), 1);
        assert_eq!(
            events.trigger_command_fires[0].commands,
            vec![
                crate::trigger_bindings::BoundTriggerCommandKind::Mover,
                crate::trigger_bindings::BoundTriggerCommandKind::Damage,
                crate::trigger_bindings::BoundTriggerCommandKind::AnimationState,
                crate::trigger_bindings::BoundTriggerCommandKind::StoreSlot,
                crate::trigger_bindings::BoundTriggerCommandKind::Arm,
            ],
            "each consequential command must cross the fixed-tick boundary once; final mover, arm, slot, and animation state alone are idempotent"
        );
        let registry_ref = registry.borrow();
        assert!(
            registry_ref
                .get_component::<KinematicMoverComponent>(mover)
                .unwrap()
                .started
        );
        assert_eq!(
            registry_ref
                .get_component::<HealthComponent>(animated_target)
                .unwrap()
                .current,
            90.0
        );
        assert_eq!(
            registry_ref
                .get_component::<MeshComponent>(animated_target)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "attack"
        );
        assert!(
            registry_ref
                .get_component::<TriggerVolumeComponent>(arm_target)
                .unwrap()
                .armed
        );
        drop(registry_ref);
        assert_eq!(
            script_ctx
                .slot_table
                .borrow()
                .get("trigger.flag")
                .unwrap()
                .value,
            Some(SlotValue::Number(1.0))
        );

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);
        let residual = bindings.residual(events.trigger_residuals[0]).unwrap();
        let _ = fire_prepartitioned_reactions_with_sequences(
            residual.steps(),
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert!(matches!(
            script_ctx.system_commands.take().as_slice(),
            [postretro_entities::SystemReactionCommand::FlashScreen { .. }]
        ));

        let _ = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            None,
            0.0,
            &mut progress,
            &mut ai_warned,
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            Some(TriggerTickContext {
                system: &mut trigger_system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: script_ctx.slot_table.clone(),
                use_edges: &use_edges,
            }),
        );
        let registry_ref = registry.borrow();
        assert!(
            registry_ref
                .get_component::<Transform>(mover)
                .unwrap()
                .position
                .x
                > 0.0,
            "the tick after same-tick mover start advances the mover without an app loop"
        );
        assert_eq!(
            registry_ref
                .get_component::<HealthComponent>(animated_target)
                .unwrap()
                .current,
            90.0,
            "the enter edge does not execute consequential work twice"
        );
    }

    #[test]
    fn trigger_commands_recheck_later_same_tick_gates_in_stable_edge_order() {
        for (command, second_starts_armed, expected_event_names, expected_health) in [
            ("disarmTrigger", true, vec!["first"], 100.0_f32),
            ("armTrigger", false, vec!["first", "second"], 95.0_f32),
        ] {
            let script_ctx = postretro_entities::ScriptCtx::new();
            let registry = script_ctx.registry.clone();
            let (first, second, target) = {
                let mut registry = registry.borrow_mut();
                let player = registry.spawn(Transform::default());
                registry
                    .set_component(player, trigger_movement())
                    .expect("player movement attaches");

                // Spawn order is the authored total order: first's command must
                // settle before the second trigger's gate is evaluated.
                let first = registry.spawn(Transform::default());
                registry
                    .set_component(first, trigger_component("first", true))
                    .expect("first trigger attaches");
                let second = registry.spawn(Transform::default());
                registry
                    .set_tags(second, vec!["second-trigger".into()])
                    .expect("second trigger accepts tag");
                registry
                    .set_component(second, trigger_component("second", second_starts_armed))
                    .expect("second trigger attaches");
                let target = registry.spawn(Transform::default());
                registry
                    .set_tags(target, vec!["damage-target".into()])
                    .expect("damage target accepts tag");
                registry
                    .set_component(
                        target,
                        HealthComponent {
                            max: 100.0,
                            current: 100.0,
                            hitbox: None,
                            death_handled: false,
                            zone_multipliers: Default::default(),
                            contributor_ledger: Default::default(),
                        },
                    )
                    .expect("damage target health attaches");
                (first, second, target)
            };
            let mut data = DataRegistry::new();
            data.populate_level(
                vec![
                    NamedReaction {
                        name: "first".into(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: command.into(),
                            tag: Some("second-trigger".into()),
                            on_complete: None,
                            args: serde_json::json!({}),
                        }),
                    },
                    NamedReaction {
                        name: "second".into(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: "applyDamage".into(),
                            tag: Some("damage-target".into()),
                            on_complete: None,
                            args: serde_json::json!({ "amount": 5 }),
                        }),
                    },
                ],
                Vec::new(),
                &[],
            );
            let bindings = TriggerBindingTable::build(
                &registry.borrow(),
                &data,
                &script_ctx.slot_table.borrow(),
            );
            let mut bridge = TriggerVolumeBridge::new();
            for trigger in [first, second] {
                bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
            }
            let mut trigger_system = TriggerSystem::default();
            let mut progress = ProgressTracker::new();
            let mut ai_warned = HashSet::new();
            let mut mover_states = MoverTickStateTable::default();
            let use_edges = HashMap::new();

            let events = simulate_tick(
                registry.clone(),
                &CollisionWorld::new(),
                &HitZoneStore::new(),
                None,
                -9.81,
                None,
                0.0,
                &mut progress,
                &mut ai_warned,
                &[],
                &mut mover_states,
                &[],
                &sim_command(false, false),
                |_| PostMovementCommand {
                    aim_origin: Vec3::ZERO,
                    aim_direction: Vec3::NEG_Z,
                },
                1.0 / 60.0,
                Some(TriggerTickContext {
                    system: &mut trigger_system,
                    bridge: &bridge,
                    bindings: &bindings,
                    slot_table: script_ctx.slot_table.clone(),
                    use_edges: &use_edges,
                }),
            );

            assert_eq!(
                events
                    .trigger_fires
                    .iter()
                    .map(|event| event.fire.event_name.as_str())
                    .collect::<Vec<_>>(),
                expected_event_names,
                "{command} must affect the later trigger gate within this tick"
            );
            let observed_health = registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("damage target remains present")
                .current;
            assert!(
                (observed_health - expected_health).abs() <= 1.0e-6,
                "the second trigger's consequential work must match the rechecked gate; expected {expected_health}, observed {observed_health}"
            );
            assert_eq!(
                registry
                    .borrow()
                    .get_component::<TriggerVolumeComponent>(second)
                    .expect("second trigger remains present")
                    .armed,
                command == "armTrigger"
            );
        }
    }

    #[test]
    fn remote_fire_authorizes_shot_and_does_not_damage_by_raycast() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon, target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(weapon, weapon_component("weapon.test.remote"))
                .unwrap();
            let target = registry.spawn(Transform::default());
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: None,
                        death_handled: false,
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
        assert_eq!(events.authorized_shots[0].owner_client_id, 7);
        let registry = registry.borrow();
        let weapon_state = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(weapon_state.cooldown_remaining_ms, 100.0);
        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 100.0);
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
        assert_eq!(events.authorized_shots[0].shot.pawn, pawn_a);
        assert_eq!(events.authorized_shots[1].shot.pawn, pawn_b);
        assert_ne!(
            events.authorized_shots[0].shot.shot_id,
            events.authorized_shots[1].shot.shot_id
        );
        let registry = registry.borrow();
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_a)
                .unwrap()
                .cooldown_remaining_ms,
            100.0
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_b)
                .unwrap()
                .cooldown_remaining_ms,
            100.0
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(idle_weapon)
                .unwrap()
                .cooldown_remaining_ms,
            0.0
        );
    }

    #[test]
    fn remote_reload_delivery_routes_to_mapped_weapon_only() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b) = {
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
            (pawn_a, weapon_a, pawn_b, weapon_b)
        };

        let events = run_remote_only_tick(
            registry,
            &[
                remote_command(pawn_a, Some(weapon_a), 10, 5, false, true),
                remote_command(pawn_b, Some(weapon_b), 11, 5, false, false),
            ],
        );

        assert_eq!(
            events.reload_deliveries,
            vec![ReloadDelivery {
                pawn: pawn_a,
                weapon: weapon_a
            }]
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

        let attacker = Some(registry.spawn(Transform::default()));
        apply_weapon_impact_damage(&mut registry, Some(weapon_id), attacker, &impact);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 75.0);
        let entries = health.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.source_id, "weapon.test.rifle");
        assert_eq!(entry.accumulated_damage, 25.0);
        assert_eq!(entry.last_hit_damage, 25.0);
        assert_eq!(entry.last_hit_zone.as_deref(), Some("head"));
        assert_eq!(entry.last_weapon, Some(weapon_id));
        assert_eq!(entry.last_attacker, attacker);
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

        apply_weapon_impact_damage(&mut registry, Some(weapon_id), None, &impact);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 100.0);
        assert!(health.contributor_ledger.entries().is_empty());
        assert!(health.contributor_ledger.overflow().is_none());
    }

    #[test]
    fn authorized_remote_hit_damage_can_run_death_sweep_in_same_host_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (weapon_id, attacker, target) = {
            let mut registry = registry.borrow_mut();
            let weapon_id = registry.spawn(Transform::default());
            let attacker = registry.spawn(Transform::default());
            let target = registry.spawn(Transform::default());
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 10.0,
                        current: 10.0,
                        hitbox: None,
                        death_handled: false,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            (weapon_id, attacker, target)
        };
        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: None,
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload { amount: 10.0 }),
        };
        {
            let mut registry = registry.borrow_mut();
            apply_authorized_weapon_impact_damage(
                &mut registry,
                weapon_id,
                Some(attacker),
                &impact,
                "weapon.test.remote".to_string(),
                10.0,
            );
            assert!(
                registry.exists(target),
                "damage alone leaves death handling for the sweep hook"
            );
        }

        let mut progress = ProgressTracker::new();
        let death_events = run_death_sweep(&registry, &mut progress);

        assert!(death_events.is_empty());
        assert!(
            !registry.borrow().exists(target),
            "the narrow post-HIT sweep removes the zero-HP target before snapshots settle"
        );
    }
}
