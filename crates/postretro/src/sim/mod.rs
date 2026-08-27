// Headless fixed-tick game-state advance seam.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

mod projectile_stage;
pub(crate) mod touch;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use glam::{EulerRot, Mat4, Vec3};
use parry3d::math::{Point, Vector};

use crate::agent_steering;
use crate::collision::moving::{
    CombinedCollisionWorld, MoverCollider, MoverPoseSource, cast_ray_combined,
};
use crate::collision::{COS_WALKABLE, CollisionWorld, cast_ray};
use crate::kinematic_mover::{self, MoverTickStateTable};
use crate::movement::MovementInput;
use crate::nav::NavGraph;
use crate::netcode::{AuthorizedShot, OpenAuthorizedShot, ShotId};
use crate::scripting_systems;
use crate::scripting_systems::hit_zones::{
    HitZoneStore, model_matrix, sample_world_pose_for_probe,
};
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::touch::TouchSystem;
use crate::trigger_bindings::{TriggerBindingTable, TriggerResidualHandle};
use crate::trigger_commands::TriggerFireContext;
#[cfg(test)]
use crate::trigger_system::TriggerEvent;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};
#[cfg(test)]
use crate::weapon;
use crate::weapon::FireButtonState;
use postretro_entities::PoseInputs;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::BrainComponent;
#[cfg(test)]
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::mesh::{MeshAnimation, MeshComponent, switch_animation_state};
use postretro_entities::components::player_movement::PlayerMovementComponent;
#[cfg(test)]
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, EntityTypeDescriptor, ScriptCtx,
    SlotTable,
};
use postretro_foundation::{
    WeaponPlacementDescriptor,
    pose::{FootProbe, MAX_FEET},
};
use postretro_net::wire::NetworkId;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
pub(crate) use projectile_stage::{
    PredictedProjectileResolution, ProjectileContactEvent, advance_predicted,
};
pub(crate) use weapon_stage::{projectile_model_body_rotation, spawn_projectile};

#[derive(Debug, Clone)]
pub(crate) struct SimCommand {
    pub(crate) movement: MovementInput,
    pub(crate) fire_button: FireButtonState,
    pub(crate) reload: bool,
    /// Slot the local client declares as the source of fire. The host resolves it
    /// from pawn inventory by possession rather than from its active pointer.
    pub(crate) firing_slot: u8,
    /// Direct number-row selection is a discrete simulation command. Cursor and
    /// dwell remain input-only state.
    pub(crate) select_slot: Option<usize>,
    /// Use rising edge routed to the host-authoritative trigger stage. Kept on
    /// the full command alongside fire/reload; `MovementInput` mirrors it for the
    /// client-prediction input boundary.
    pub(crate) use_pressed: bool,
    /// Drop rising edge routed to the host-authoritative touch stage. Kept on
    /// the full command alongside `use_pressed`; `MovementInput` mirrors it for
    /// the client-prediction input boundary.
    pub(crate) drop_pressed: bool,
}

pub(crate) struct PostMovementCommand {
    pub(crate) aim_origin: Vec3,
    pub(crate) aim_direction: Vec3,
}

/// Roll back a host-refused locally-running inventory switch without entering a
/// second equip transition. The network Control drain owns when this is invoked;
/// the weapon stage owns the machine-state cleanup.
pub(crate) fn refuse_local_wieldable_switch(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    refused_slot: usize,
    rollback_slot: usize,
) -> bool {
    weapon_stage::refuse_local_switch(registry, pawn, refused_slot, rollback_slot)
}

/// Clear despawned wieldable slots and surface pawns whose active instance changed.
/// The caller feeds those pawns into the existing attachment-dirty path.
pub(crate) fn normalize_wieldable_inventories(registry: &mut EntityRegistry) -> Vec<EntityId> {
    weapon_stage::normalize_all_inventory_liveness(registry)
}

/// Normalize one pawn before a host declaration and report whether its active
/// instance changed so presentation dirtiness follows the same liveness result.
pub(crate) fn normalize_wieldable_inventory(
    registry: &mut EntityRegistry,
    pawn: EntityId,
) -> Option<(postretro_entities::components::inventory::Inventory, bool)> {
    weapon_stage::normalize_inventory_liveness(registry, pawn)
}

/// Advance only the connected client's local wieldable machine. Movement stays on
/// the prediction path and authoritative combat stays on the host; this pass owns
/// the immediate local lower/raise/repoint and suppresses fire while doing so.
#[allow(clippy::too_many_arguments)]
pub(crate) fn simulate_client_wieldable_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    pawn: Option<EntityId>,
    mod_block_during_reload: bool,
    select_slot: Option<usize>,
    fire_button: crate::weapon::FireButtonState,
    reload_held: bool,
    anim_time: f64,
    tick_dt: f32,
) -> (bool, Option<EntityId>) {
    let mut equip_was_active = false;
    let mut requested_new_slot = false;
    let cooldown_before = pawn.and_then(|pawn| {
        let registry = registry.borrow();
        let inventory = registry
            .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
            .ok()?;
        requested_new_slot = select_slot.is_some_and(|slot| {
            slot != inventory.active_slot
                && inventory.switch_target != Some(slot)
                && inventory.wieldables.get(slot).copied().flatten().is_some()
        });
        let weapon = inventory.active_wieldable()?;
        let component = registry
            .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon)
            .ok()?;
        equip_was_active = matches!(
            component.state,
            postretro_entities::components::wieldable_state::WieldableState::Lowering
                | postretro_entities::components::wieldable_state::WieldableState::Raising
        );
        let cooldown = component.cooldown_remaining_ms;
        Some((weapon, cooldown))
    });
    let machine_button = if select_slot.is_some() || equip_was_active {
        fire_button
    } else {
        crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        }
    };
    let machine_reload = (select_slot.is_some() || equip_was_active) && reload_held;
    let command = crate::weapon::WeaponFireCommand {
        button: machine_button,
        aim_origin: Vec3::ZERO,
        aim_direction: Vec3::Z,
        can_fire: false,
    };
    let mut ignore_impact = |_: &mut EntityRegistry| {};
    let result = weapon_stage::run_local_weapon_command(
        &registry,
        pawn,
        mod_block_during_reload,
        select_slot,
        &command,
        machine_reload,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        &mut ignore_impact,
    );
    // Client fire prediction advances cooldown once at render rate after the
    // fixed-tick loop. Keep this equip-only pass from charging the same elapsed
    // time twice while preserving deploy clamps on the incoming instance.
    if let Some((weapon, cooldown)) = cooldown_before {
        let mut registry = registry.borrow_mut();
        if let Ok(mut component) = registry
            .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon)
            .cloned()
        {
            component.cooldown_remaining_ms = cooldown;
            let _ = registry.set_component(weapon, component);
        }
    }
    let accepted = requested_new_slot
        && match (pawn, select_slot) {
            (Some(pawn), Some(slot)) => registry
                .borrow()
                .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
                .is_ok_and(|inventory| {
                    inventory.switch_target == Some(slot) || inventory.active_slot == slot
                }),
            _ => false,
        };
    (accepted, result.repointed_pawn)
}

fn player_is_present_for_trigger_occupancy(
    registry: &EntityRegistry,
    player: &AuthoritativePlayer,
) -> bool {
    registry.exists(player.pawn)
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
    pub(crate) aim_pitch: f32,
    pub(crate) command: SimCommand,
}

/// A host-only presentation launch for an accepted connected-client projectile
/// fire. The authoritative hit remains client-declared; this is only the data the
/// host needs to show that flight to observers through the existing snapshot path.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RemoteProjectilePresentationLaunch {
    pub(crate) owner_client_id: u64,
    pub(crate) shot_id: ShotId,
    pub(crate) origin: Vec3,
    pub(crate) direction: Vec3,
    pub(crate) range: f32,
    pub(crate) descriptor_class: String,
    pub(crate) projectile: postretro_foundation::ProjectileDescriptor,
}

/// Host-resolved projectile FIRE refusal. It reuses the existing owner-private
/// ShotVerdict wire fact so the client can stop its matching predicted flight
/// without waiting for a later impact or expiry declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemoteProjectileFireRejection {
    pub(crate) owner_client_id: u64,
    pub(crate) shot_id: ShotId,
}

/// Host-only inputs for the trigger stage. The system itself consumes the
/// per-player map, never an action snapshot; local and remote use edges are
/// keyed by `PlayerId` at this boundary.
pub(crate) struct TriggerTickContext<'a> {
    pub(crate) system: &'a mut TriggerSystem,
    pub(crate) bridge: &'a TriggerVolumeBridge,
    pub(crate) bindings: &'a TriggerBindingTable,
    pub(crate) slot_table: Rc<RefCell<SlotTable>>,
    /// Present for production level installs and IR-aware harnesses. Literal
    /// fixtures may omit it and keep their direct table execution path.
    pub(crate) script_ctx: Option<ScriptCtx>,
    /// Host-only auto-close timer side table. Omitted by lightweight fixtures
    /// and never constructed for connected-client prediction.
    pub(crate) auto_close_timers: Option<kinematic_mover::MoverAutoCloseTimers>,
    pub(crate) use_edges: &'a HashMap<PlayerId, bool>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct TickEvents {
    pub(crate) movement: Vec<&'static str>,
    /// AI events raised this tick: the static enemy-attack address plus each
    /// entered graph state's authored `on_enter`, which is owned.
    pub(crate) ai: Vec<Cow<'static, str>>,
    pub(crate) weapon: Vec<&'static str>,
    /// Per-pellet cast points for determinism tests. Capture precedes impact
    /// policy, so tests compare the cast set rather than the applied subset.
    #[cfg(test)]
    pub(crate) weapon_impact_points: Vec<Vec3>,
    /// Host-local kinematic mover transition edges. A connected client never
    /// runs the host simulation seam, so its bucket remains empty.
    pub(crate) mover: Vec<(kinematic_mover::MoverEventKind, u32)>,
    pub(crate) death: Vec<String>,
    pub(crate) authorized_shots: Vec<OpenAuthorizedShot>,
    /// Host-only remote-fire visual launches. No component here is gameplay
    /// authoritative and nothing in this event crosses the wire directly.
    pub(crate) remote_projectile_presentation_launches: Vec<RemoteProjectilePresentationLaunch>,
    /// Prompt owner-private corrections for projectile FIRE attempts rejected by
    /// the host weapon gate. Accepted hitscan and pellet verdict timing is unchanged.
    pub(crate) rejected_remote_projectile_fires: Vec<RemoteProjectileFireRejection>,
    /// Locally simulated projectiles that a listen host mirrors for remote observers.
    /// The host's renderer suppresses the mirror and continues to draw this source.
    pub(crate) local_projectile_spawns: Vec<EntityId>,
    /// Locally simulated projectile contacts that retire listen-host mirror flights.
    pub(crate) local_projectile_contacts: Vec<ProjectileContactEvent>,
    pub(crate) reload_deliveries: Vec<ReloadDelivery>,
    /// Pawns whose active inventory slot repointed this tick. Presentation drains
    /// this after simulation so the hand socket follows committed ownership, never
    /// a pending selection.
    pub(crate) repointed_pawns: Vec<EntityId>,
    /// The host drop path restores meshes outside spawn-context resolution, so it
    /// reports them here for clip binding in the same fixed tick, before rendering.
    pub(crate) dropped_item_meshes: Vec<EntityId>,
    /// Bound trigger residuals drained app-side after every fixed tick this frame,
    /// each carried with the `(trigger, player)` that fired it. The origin rides
    /// alongside the opaque handle so the frame-end drain can key an enrolled
    /// timed-reaction instance to its activator — two players entering one plate
    /// push the same handle twice and must not collapse into one instance (O59).
    pub(crate) trigger_residuals: Vec<(TriggerResidualHandle, EntityId, PlayerId)>,
    /// This tick's paired-trigger Exit fires, as `(trigger, player)`. Production
    /// reads them in the `RedrawRequested` arm to cancel matching interruptible
    /// timed-reaction instances before their countdown advances (O4). Derived from
    /// `TriggerEvent.fire` filtered by `edge == Exit`; a non-`cfg(test)` field
    /// because the scheduler consumes it in release, unlike `trigger_fires`.
    pub(crate) trigger_exit_fires: Vec<(EntityId, PlayerId)>,
    /// Test-only fixed-tick trace. Production consumes residual handles only;
    /// keeping the detailed sequence out of non-test builds avoids a hot-path
    /// diagnostic allocation.
    #[cfg(test)]
    pub(crate) trigger_fires: Vec<TriggerEvent>,
    #[cfg(test)]
    pub(crate) trigger_command_fires: Vec<TriggerCommandFire>,
}

/// Advance transient presentation work on a connected client. Connected clients
/// do not run [`simulate_tick`], so their predicted and observer-materialized
/// impact lights need an explicit per-frame deferred-effect queue boundary.
pub(crate) fn advance_client_presentation_effects(registry: &mut EntityRegistry, frame_dt: f32) {
    crate::impact_effects::tick_deferred_effects(registry, frame_dt);
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerCommandFire {
    pub(crate) event: TriggerEvent,
    pub(crate) commands: Vec<crate::trigger_bindings::BoundTriggerCommandKind>,
}

#[allow(clippy::too_many_arguments, dead_code)]
pub(crate) fn simulate_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    gravity: f32,
    _legacy_active_wieldable: Option<EntityId>,
    anim_time: f64,
    _progress_tracker: &mut ProgressTracker,
    ai_runtime: &mut scripting_systems::ai::AiRuntime,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_commands: &[RemotePawnCommand],
    command: &SimCommand,
    post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
    trigger_context: Option<TriggerTickContext<'_>>,
    on_impact: impl FnMut(&mut EntityRegistry),
) -> TickEvents {
    let mut touch_system = TouchSystem::default();
    let touch_edges = HashMap::new();
    simulate_tick_with_presentation_aim(
        registry,
        collision_world,
        hit_zone_store,
        nav_graph,
        gravity,
        false,
        anim_time,
        (0.0, 0.0),
        _progress_tracker,
        ai_runtime,
        mover_colliders,
        mover_tick_states,
        remote_pawn_commands,
        command,
        post_movement,
        tick_dt,
        &mut touch_system,
        &[],
        None,
        &touch_edges,
        &touch_edges,
        trigger_context,
        on_impact,
    )
}

/// Fixed-tick simulation with the render-assembly camera aim captured by the App.
/// Headless callers retain the wrapper above and intentionally use the neutral
/// camera; production local-player presentation passes the live camera aim here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn simulate_tick_with_presentation_aim(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    gravity: f32,
    mod_block_during_reload: bool,
    anim_time: f64,
    presentation_camera_aim: (f32, f32),
    _progress_tracker: &mut ProgressTracker,
    ai_runtime: &mut scripting_systems::ai::AiRuntime,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_commands: &[RemotePawnCommand],
    command: &SimCommand,
    mut post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
    touch_system: &mut TouchSystem,
    descriptors: &[EntityTypeDescriptor],
    default_weapon_placement: Option<&WeaponPlacementDescriptor>,
    use_pressed: &HashMap<PlayerId, bool>,
    drop_pressed: &HashMap<PlayerId, bool>,
    trigger_context: Option<TriggerTickContext<'_>>,
    mut on_impact: impl FnMut(&mut EntityRegistry),
) -> TickEvents {
    registry.borrow_mut().snapshot_transforms();

    // This is the fixed-tick queue boundary. Producers run later in this tick
    // (AI, local weapon fire, and host remote-hit ingest after simulate_tick),
    // so every newly queued effect keeps its full authored delay until the
    // next fixed tick, including in headless simulation.
    {
        let mut registry = registry.borrow_mut();
        crate::impact_effects::tick_deferred_effects(&mut registry, tick_dt);
    }

    let auto_close_timers = trigger_context
        .as_ref()
        .and_then(|context| context.auto_close_timers.clone());
    {
        let mut registry = registry.borrow_mut();
        kinematic_mover::run_kinematic_mover_tick(&mut registry, mover_tick_states, tick_dt);
    }
    let mut mover_events: Vec<_> = mover_tick_states.terminus_events().collect();
    if let Some(auto_close_timers) = auto_close_timers.as_ref() {
        auto_close_timers.arm_opened_termini(&mut registry.borrow_mut(), &mover_events);
    }

    let remote_pawn_inputs: Vec<(EntityId, MovementInput)> = remote_pawn_commands
        .iter()
        .map(|remote| (remote.pawn, remote.command.movement.clone()))
        .collect();
    let remote_player_aims: HashMap<EntityId, (f32, f32)> = remote_pawn_commands
        .iter()
        .map(|remote| {
            (
                remote.pawn,
                (remote.aim_pitch, remote.command.movement.facing_yaw),
            )
        })
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
    let mut players: Vec<AuthoritativePlayer> = remote_pawn_commands
        .iter()
        .map(|remote| AuthoritativePlayer {
            id: PlayerId::Remote(remote.owner_client_id),
            pawn: remote.pawn,
        })
        .collect();
    let local_player = {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    if let Some(pawn) = local_player {
        players.push(AuthoritativePlayer {
            id: PlayerId::Local(pawn),
            pawn,
        });
    }

    let mut trigger_residuals = Vec::new();
    let mut trigger_exit_fires: Vec<(EntityId, PlayerId)> = Vec::new();
    #[cfg(test)]
    let mut trigger_fires = Vec::new();
    #[cfg(test)]
    let mut trigger_command_fires = Vec::new();
    if let Some(trigger_context) = trigger_context {
        let canonical_player_pawns = {
            let registry = registry.borrow();
            crate::trigger_system::canonical_player_pawns(&registry, &players)
        };
        let alive_players: HashSet<PlayerId> = {
            let registry = registry.borrow();
            players
                .iter()
                .filter(|player| player_is_present_for_trigger_occupancy(&registry, player))
                .map(|player| player.id)
                .collect()
        };
        let mut registry = registry.borrow_mut();
        let bound_edges = trigger_context.bindings.bound_edges();
        if let Some(script_ctx) = trigger_context.script_ctx.as_ref() {
            let dispatch_inputs = crate::trigger_system::TriggerDispatchInputs {
                alive_players: &alive_players,
                bound_edges,
            };
            let _report = trigger_context.system.run_authoritative_tick_with_dispatch(
                &mut registry,
                trigger_context.bridge,
                crate::trigger_system::TriggerTickInputs {
                    players: &players,
                    use_pressed: trigger_context.use_edges,
                    tick_dt,
                },
                dispatch_inputs,
                |event, occupancy, registry| {
                    let fire_context =
                        trigger_fire_context(event, occupancy, &canonical_player_pawns);
                    let execution = trigger_context.bindings.execute_with_script_ctx(
                        event.fire.trigger,
                        event.edge,
                        registry,
                        script_ctx,
                        &fire_context,
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
                        // Carry the firing origin beside the handle so the
                        // frame-end drain keys each enrolled instance to its
                        // activator; one handle fired by two players yields two
                        // origins, not one collapsed instance (O59).
                        trigger_residuals.push((handle, event.fire.trigger, event.fire.player));
                    }
                    if event.edge == crate::trigger_system::TriggerEventEdge::Exit {
                        trigger_exit_fires.push((event.fire.trigger, event.fire.player));
                    }
                },
            );
        } else {
            let mut slot_table = trigger_context.slot_table.borrow_mut();
            let dispatch_inputs = crate::trigger_system::TriggerDispatchInputs {
                alive_players: &alive_players,
                bound_edges,
            };
            let _report = trigger_context.system.run_authoritative_tick_with_dispatch(
                &mut registry,
                trigger_context.bridge,
                crate::trigger_system::TriggerTickInputs {
                    players: &players,
                    use_pressed: trigger_context.use_edges,
                    tick_dt,
                },
                dispatch_inputs,
                |event, _occupancy, registry| {
                    let fire_context = TriggerFireContext::default();
                    let execution = trigger_context.bindings.execute(
                        event.fire.trigger,
                        event.edge,
                        registry,
                        &mut slot_table,
                        &fire_context,
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
                        // Carry the firing origin beside the handle so the
                        // frame-end drain keys each enrolled instance to its
                        // activator; one handle fired by two players yields two
                        // origins, not one collapsed instance (O59).
                        trigger_residuals.push((handle, event.fire.trigger, event.fire.player));
                    }
                    if event.edge == crate::trigger_system::TriggerEventEdge::Exit {
                        trigger_exit_fires.push((event.fire.trigger, event.fire.player));
                    }
                },
            );
        }
    }
    let touch_events = {
        let mut registry = registry.borrow_mut();
        touch_system.run_authoritative_tick(
            &mut registry,
            collision_world,
            descriptors,
            &players,
            use_pressed,
            drop_pressed,
        )
    };
    let ai = {
        let mut registry = registry.borrow_mut();
        scripting_systems::ai::run_ai_tick_with_navigation_and_impact(
            &mut registry,
            ai_runtime,
            tick_dt,
            nav_graph,
            Some(collision_world),
            &mut on_impact,
        )
    };

    let post_movement_command = post_movement(&registry);

    {
        let mut registry = registry.borrow_mut();
        // AgentTickResult only carries a diagnostic `replans` counter, not observable sim state, so the return value is intentionally discarded.
        let _ = agent_steering::tick(&mut registry, collision_world, nav_graph, gravity, tick_dt);
        update_player_animation_locomotion(&mut registry, hit_zone_store, anim_time);
        update_brain_animation_playback_rates(&mut registry, hit_zone_store, anim_time);
        update_presentation_pose_inputs(
            &mut registry,
            collision_world,
            mover_colliders,
            &*mover_tick_states,
            hit_zone_store,
            anim_time,
            PresentationPoseInputs {
                camera_aim: presentation_camera_aim,
                remote_player_aims: &remote_player_aims,
                remote_aim_pitches: &HashMap::new(),
                remote_heading_yaws: &HashMap::new(),
                remote_network_ids: &HashMap::new(),
            },
        );
        if let Some(auto_close_timers) = auto_close_timers.as_ref() {
            auto_close_timers.tick(&mut registry, tick_dt);
        }
        let (mover_poses, blocking_state) = mover_tick_states.split_for_blocking();
        kinematic_mover::run_mover_blocking_pass(
            &mut registry,
            collision_world,
            mover_colliders,
            &mover_poses,
            blocking_state,
            tick_dt,
            &mut mover_events,
            &mut on_impact,
        );
    }

    let remote_weapon_result =
        weapon_stage::run_remote_weapon_commands(&registry, remote_pawn_commands, tick_dt);
    let own_pawn = {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    let weapon_fire = weapon_stage::weapon_fire_command(command.fire_button, post_movement_command);
    let local_result: weapon_stage::LocalWeaponCommandResult =
        weapon_stage::run_local_weapon_command_with_content(
            &registry,
            own_pawn,
            mod_block_during_reload,
            descriptors,
            default_weapon_placement,
            command.select_slot,
            &weapon_fire,
            command.reload,
            collision_world,
            hit_zone_store,
            anim_time,
            tick_dt,
            &mut on_impact,
        );
    let mut reload_deliveries = remote_weapon_result.reload_deliveries;
    reload_deliveries.extend(local_result.reload_deliveries);
    let mut weapon = local_result.weapon_events;
    let repointed_pawn = local_result.repointed_pawn;
    #[cfg(test)]
    let weapon_impact_points = local_result.weapon_impact_points;
    weapon.extend(remote_weapon_result.weapon_events);
    let local_projectile_contacts = projectile_stage::advance(
        &registry,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        &mut on_impact,
    );
    let death = run_death_sweep(&registry);

    let mut repointed_pawns = touch_events.repointed_pawns;
    if let Some(pawn) = repointed_pawn {
        repointed_pawns.push(pawn);
    }
    repointed_pawns.sort_unstable();
    repointed_pawns.dedup();

    TickEvents {
        movement,
        ai,
        weapon,
        #[cfg(test)]
        weapon_impact_points,
        mover: mover_events,
        death,
        authorized_shots: remote_weapon_result.authorized_shots,
        remote_projectile_presentation_launches: remote_weapon_result
            .projectile_presentation_launches,
        rejected_remote_projectile_fires: remote_weapon_result.rejected_projectile_fires,
        local_projectile_spawns: local_result.projectile_spawns,
        local_projectile_contacts,
        reload_deliveries,
        repointed_pawns,
        dropped_item_meshes: touch_events.dropped_item_meshes,
        trigger_residuals,
        trigger_exit_fires,
        #[cfg(test)]
        trigger_fires,
        #[cfg(test)]
        trigger_command_fires,
    }
}

fn trigger_fire_context(
    event: &crate::trigger_system::TriggerEvent,
    occupancy: usize,
    canonical_player_pawns: &BTreeMap<PlayerId, EntityId>,
) -> TriggerFireContext {
    let activator = match event.fire.player {
        PlayerId::Local(pawn) => Some(pawn),
        PlayerId::Remote(_) => canonical_player_pawns.get(&event.fire.player).copied(),
    };
    if activator.is_none() && matches!(event.fire.player, PlayerId::Remote(_)) {
        log::warn!(
            "[Trigger] remote activator {:?} is absent from this tick; @activators is empty",
            event.fire.player
        );
    }
    TriggerFireContext {
        fired_trigger: Some(event.fire.trigger),
        activator,
        occupancy,
    }
}

/// Select authoritative player locomotion after movement resolves. Remote-owned host
/// pawns carry no brain, so without this pass their descriptor default (`idle`) would
/// be serialized forever and repeatedly correct the client's velocity prediction.
/// State selection and rate calibration intentionally match the client presentation
/// path; ordinary switches use the authored crossfade, preserving no-snap correction.
pub(crate) fn update_player_animation_locomotion(
    registry: &mut EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    const MOVING_SPEED_EPSILON: f32 = 1.0e-4;

    let players: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .map(|(id, _)| id)
        .collect();
    for id in players {
        let Ok(movement) = registry.get_component::<PlayerMovementComponent>(id) else {
            continue;
        };
        let speed_xz = Vec3::new(movement.velocity.x, 0.0, movement.velocity.z).length();
        let walk_speed = movement.ground_params.speed.walk;
        let run_speed = movement.ground_params.speed.run;

        let (idle_state, walk_state, run_state, current_state) = {
            let Ok(mesh) = registry.get_component::<MeshComponent>(id) else {
                continue;
            };
            let Some(animation) = mesh.animation.as_ref() else {
                continue;
            };
            let walk_state = ["walk_forward", "walk"]
                .into_iter()
                .find(|state| animation.states.contains_key(*state))
                .map(str::to_string);
            let Some(walk_state) = walk_state else {
                continue;
            };
            (
                animation.default_state.clone(),
                walk_state,
                animation
                    .states
                    .contains_key("run")
                    .then(|| "run".to_string()),
                animation.current_state.clone(),
            )
        };
        let moving = speed_xz.is_finite() && speed_xz > MOVING_SPEED_EPSILON;
        let (target_state, fallback_speed) = if !moving {
            (idle_state, 1.0)
        } else if speed_xz > walk_speed && run_state.is_some() {
            (run_state.expect("checked above"), run_speed)
        } else {
            (walk_state, walk_speed)
        };
        if current_state != target_state {
            let _ = switch_animation_state(registry, id, &target_state);
        }

        let rate_input = registry
            .get_component::<MeshComponent>(id)
            .ok()
            .and_then(|mesh| {
                let animation = mesh.animation.as_ref()?;
                if !moving || animation.current_state != target_state || !animation.speed_scale {
                    return Some(1.0);
                }
                let effective = effective_travel_speed(animation, mesh, hit_zone_store);
                Some(MeshAnimation::locomotion_rate_ratio(
                    speed_xz.max(0.0),
                    effective,
                    fallback_speed,
                ))
            });
        let Some(rate_input) = rate_input else {
            continue;
        };
        let needs_rebase = registry
            .get_component::<MeshComponent>(id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .is_some_and(|animation| animation.playback_rate_needs_update(rate_input));
        if !needs_rebase {
            continue;
        }
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

/// Update brain-driven walk playback after steering has resolved this tick's
/// velocity. `anim_time` is the slow-mo/freeze-gated animation clock sampled by
/// the renderer. The AI pass's locomotion intent is deliberately not reused: it
/// is a pre-steering, squared-speed read from the prior tick, while this path
/// must match the motion the steering system actually produced.
fn update_brain_animation_playback_rates(
    registry: &mut EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
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

        let Ok(mesh) = registry.get_component::<MeshComponent>(id) else {
            continue;
        };
        let Some(animation) = mesh.animation.as_ref() else {
            continue;
        };

        // Rate-scale only the graph's locomotion state — the one that chases
        // without an action of its own — and only while the archetype leaves
        // `speedScale` on. Every other case rests at the authored rate (1.0).
        // Calibration is `measured_ground_speed / effective_travel_speed`; a
        // state with neither an override nor a derived clip stride falls back to
        // `speed_xz / move_speed`, keeping the shipped in-place walk unchanged.
        let is_locomotion = scripting_systems::ai::locomotion_animation(&brain.graph)
            .is_some_and(|locomotion| animation.current_state == locomotion);
        let rate_input = if is_locomotion && animation.speed_scale {
            let effective = effective_travel_speed(animation, mesh, hit_zone_store);
            MeshAnimation::locomotion_rate_ratio(speed_xz, effective, agent.move_speed)
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

/// Resolve the effective travel speed for a locomotion state's speed-scaled
/// playback: the active state's authored `travelSpeed` override wins, else the
/// model clip's load-derived stride (via [`HitZoneStore`], keyed by the mesh's
/// model handle and active state's clip). Runtime spawns may still have a pending
/// clip index during their first rate pass, so the declared clip name is the
/// equivalent lookup fallback. `None` when neither is calibrated — the
/// degenerate reference (`speed_xz / move_speed`) that keeps the shipped
/// in-place walk byte-for-byte unchanged.
fn effective_travel_speed(
    animation: &MeshAnimation,
    mesh: &MeshComponent,
    hit_zone_store: &HitZoneStore,
) -> Option<f32> {
    let state = animation.states.get(&animation.current_state)?;
    let derived = hit_zone_store.get_by_name(&mesh.model).and_then(|model| {
        state
            .clip_index
            .and_then(|clip_index| model.clips.get(clip_index))
            // Runtime spawns reach this pass before their install-time clip-index
            // queue drains. Resolve by declared clip name for that first tick so
            // derived calibration does not temporarily fall back to move_speed.
            .or_else(|| model.clips.iter().find(|clip| clip.name == state.clip))
            .and_then(|clip| clip.travel_speed)
    });
    state.effective_travel_speed(derived)
}

/// Produce renderer-facing pose inputs from the registry's currently displayed
/// transforms. The fixed tick calls this after steering; connected clients call
/// it after remote interpolation because they intentionally skip `simulate_tick`.
/// This mutates presentation-only mesh inputs and never enters replication.
pub(crate) struct PresentationPoseInputs<'a> {
    pub(crate) camera_aim: (f32, f32),
    pub(crate) remote_player_aims: &'a HashMap<EntityId, (f32, f32)>,
    pub(crate) remote_aim_pitches: &'a HashMap<NetworkId, f32>,
    pub(crate) remote_heading_yaws: &'a HashMap<NetworkId, f32>,
    pub(crate) remote_network_ids: &'a HashMap<EntityId, NetworkId>,
}

pub(crate) fn update_presentation_pose_inputs(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    presentation: PresentationPoseInputs<'_>,
) {
    let PresentationPoseInputs {
        camera_aim,
        remote_player_aims,
        remote_aim_pitches,
        remote_heading_yaws,
        remote_network_ids,
    } = presentation;
    update_pose_inputs(
        registry,
        camera_aim,
        remote_player_aims,
        remote_aim_pitches,
        remote_heading_yaws,
        remote_network_ids,
    );
    update_foot_ground_probes(
        registry,
        collision_world,
        mover_colliders,
        mover_poses,
        hit_zone_store,
        anim_time,
    );
}

/// Write same-tick presentation inputs after AI and steering have settled the
/// entity's target and body rotation. Every animated mesh receives a finite
/// value; entities without a live acquired target hold their body heading with
/// zero pitch, making pose modifiers a visual no-op.
fn update_pose_inputs(
    registry: &mut EntityRegistry,
    camera_aim: (f32, f32),
    remote_player_aims: &HashMap<EntityId, (f32, f32)>,
    remote_aim_pitches: &HashMap<NetworkId, f32>,
    remote_heading_yaws: &HashMap<NetworkId, f32>,
    remote_network_ids: &HashMap<EntityId, NetworkId>,
) {
    const MIN_HORIZONTAL_LEN_SQ: f32 = 1e-8;

    let animated: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, _)| {
            registry
                .get_component::<MeshComponent>(id)
                .ok()
                .is_some_and(|mesh| mesh.animation.is_some())
                .then_some(id)
        })
        .collect();

    for id in animated {
        let Ok(transform) = registry
            .get_component::<postretro_entities::Transform>(id)
            .copied()
        else {
            continue;
        };
        let (raw_heading, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
        let heading_yaw = if raw_heading.is_finite() {
            raw_heading
        } else {
            0.0
        };

        let target_position = registry
            .get_component::<BrainComponent>(id)
            .ok()
            .and_then(|brain| brain.acquired_target)
            .and_then(|target| {
                registry
                    .get_component::<postretro_entities::Transform>(target)
                    .ok()
                    .map(|transform| transform.position)
            });

        let (default_aim_pitch, default_aim_yaw) = target_position
            .map(|target| target - transform.position)
            .filter(|direction| direction.is_finite())
            .map(|direction| {
                let horizontal_len_sq = direction.x * direction.x + direction.z * direction.z;
                let aim_yaw = if horizontal_len_sq > MIN_HORIZONTAL_LEN_SQ {
                    direction.x.atan2(direction.z)
                } else {
                    heading_yaw
                };
                let horizontal_len = horizontal_len_sq.max(0.0).sqrt();
                let pitch = direction
                    .y
                    .atan2(horizontal_len)
                    .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
                (
                    if pitch.is_finite() { pitch } else { 0.0 },
                    if aim_yaw.is_finite() {
                        aim_yaw
                    } else {
                        heading_yaw
                    },
                )
            })
            .unwrap_or((0.0, heading_yaw));

        let (aim_pitch, aim_yaw, heading_yaw) = if registry.local_player_movement_pawn() == Some(id)
        {
            let player_heading = registry
                .get_component::<PlayerMovementComponent>(id)
                .ok()
                .map(|movement| movement.velocity)
                .filter(|velocity| velocity.is_finite())
                .map(|velocity| player_travel_heading_yaw(velocity, heading_yaw))
                .unwrap_or(heading_yaw);
            let aim_pitch = camera_aim
                .0
                .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
            let aim_yaw = camera_aim.1;
            (
                if aim_pitch.is_finite() {
                    aim_pitch
                } else {
                    0.0
                },
                if aim_yaw.is_finite() {
                    aim_yaw
                } else {
                    player_heading
                },
                player_heading,
            )
        } else if let Some(&(aim_pitch, aim_yaw)) = remote_player_aims.get(&id) {
            let player_heading = registry
                .get_component::<PlayerMovementComponent>(id)
                .ok()
                .map(|movement| movement.velocity)
                .filter(|velocity| velocity.is_finite())
                .map(|velocity| player_travel_heading_yaw(velocity, heading_yaw))
                .unwrap_or(heading_yaw);
            (
                if aim_pitch.is_finite() {
                    aim_pitch.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
                } else {
                    0.0
                },
                if aim_yaw.is_finite() {
                    aim_yaw
                } else {
                    heading_yaw
                },
                player_heading,
            )
        } else if let Some(network_id) = remote_network_ids.get(&id)
            && let Some(&aim_pitch) = remote_aim_pitches.get(network_id)
        {
            let remote_heading = remote_heading_yaws
                .get(network_id)
                .copied()
                .filter(|yaw| yaw.is_finite())
                .unwrap_or(heading_yaw);
            (
                if aim_pitch.is_finite() {
                    aim_pitch
                } else {
                    0.0
                },
                heading_yaw,
                remote_heading,
            )
        } else {
            (default_aim_pitch, default_aim_yaw, heading_yaw)
        };

        // Single borrow serves both the change check and the clone source, so a
        // stationary/idle crowd whose inputs haven't moved skips the write
        // entirely — mirrors the discipline in
        // `update_brain_animation_playback_rates` above.
        let Ok(current) = registry.get_component::<MeshComponent>(id) else {
            continue;
        };
        // This pass owns only the aim/heading fields. The feet/foot_count are
        // authored by `update_foot_ground_probes` (ordered after this), so carry
        // them forward rather than clobbering with `..Default::default()`.
        let previous = current.pose_inputs.unwrap_or_default();
        let new_pose_inputs = PoseInputs {
            aim_pitch,
            aim_yaw,
            heading_yaw,
            feet: previous.feet,
            foot_count: previous.foot_count,
        };
        if current.pose_inputs == Some(new_pose_inputs) {
            continue;
        }
        let mut mesh = current.clone();
        mesh.pose_inputs = Some(new_pose_inputs);
        let _ = registry.set_component(id, mesh);
    }
}

/// Convert player travel velocity to the engine's yaw convention, where yaw zero
/// faces `-Z`. Stationary or non-finite input retains the supplied body heading.
pub(crate) fn player_travel_heading_yaw(velocity: Vec3, fallback: f32) -> f32 {
    const MIN_HORIZONTAL_LEN_SQ: f32 = 1e-8;
    let horizontal_len_sq = velocity.x * velocity.x + velocity.z * velocity.z;
    if velocity.is_finite() && horizontal_len_sq > MIN_HORIZONTAL_LEN_SQ {
        let yaw = (-velocity.x).atan2(-velocity.z);
        if yaw.is_finite() {
            return yaw;
        }
    }
    fallback
}

/// Model-space downward reach of each foot ground probe, in model units. Ground
/// farther than this below the animated foot reads as no contact — a swing foot
/// with no plantable surface — sized for the roughly unit-tall models the loader
/// ships. Scaled by the entity's model scale at cast time so the bound stays
/// constant in model space regardless of instance scale.
const FOOT_PLANTING_REACH: f32 = 0.5;
/// Upward model-space allowance that lets a probe recover a foot already sunk
/// slightly through the floor. The ray starts this far above the sampled foot,
/// then covers this allowance plus [`FOOT_PLANTING_REACH`] downward.
const FOOT_PENETRATION_ALLOWANCE: f32 = 0.15;

/// Sample each leg-tagged entity's UNMODIFIED world foot pose, cast a short
/// downward ray at the collision world under each foot, and write the
/// model-space contact into `PoseInputs::feet` for the renderer's IK solver.
///
/// Ordered AFTER [`update_pose_inputs`], which owns the aim/heading fields: this
/// step is the sole writer of `feet`/`foot_count` and read-modify-writes the
/// mesh's existing `pose_inputs` so those aim fields survive. Leg `i` drives foot
/// probe `i`; `foot_count` is the entity's leg-set length even when a foot finds
/// no ground (that foot reports `hit == false`). The world-pose sample is the
/// unmodified pose shared with hit zones — distinct from the renderer's modified
/// palette. Everything runs against the same fixed-tick registry/collision state
/// through the same per-instance seed, so repeated headless runs of a tick
/// sequence produce identical probes.
fn update_foot_ground_probes(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    let mut candidates = hit_zone_store.foot_probe_candidates();
    candidates.clear();
    candidates.extend(
        registry
            .iter_with_kind(ComponentKind::Mesh)
            .filter_map(|(id, value)| {
                let ComponentValue::Mesh(mesh) = value else {
                    return None;
                };
                let is_legged = hit_zone_store
                    .get_by_name(&mesh.model)
                    .is_some_and(|model| !model.legs.is_empty());
                let has_stale_feet = mesh.pose_inputs.is_some_and(|inputs| {
                    inputs.foot_count != 0
                        || inputs
                            .feet
                            .iter()
                            .any(|probe| *probe != FootProbe::default())
                });
                (is_legged || has_stale_feet).then_some(id)
            }),
    );

    for &id in candidates.iter() {
        let Ok(mesh) = registry.get_component::<MeshComponent>(id) else {
            continue;
        };
        let zones = hit_zone_store.get_by_name(&mesh.model);
        let mut foot_count = 0;
        let mut feet = [FootProbe::default(); MAX_FEET];
        if let (Some(zones), Ok(transform)) = (
            zones,
            registry
                .get_component::<postretro_entities::Transform>(id)
                .copied(),
        ) {
            if let Some(model_to_world) = model_matrix(&transform, mesh.origin_offset) {
                if let Some(world_to_model) = foot_probe_inverse(&transform, &model_to_world) {
                    let world_joints = sample_world_pose_for_probe(
                        zones,
                        mesh.animation.as_ref(),
                        anim_time,
                        id.to_raw(),
                    );
                    if let Some(world_joints) = world_joints.as_ref() {
                        foot_count = zones.legs.len().min(MAX_FEET);
                        let downward_reach = FOOT_PLANTING_REACH * transform.scale.y;
                        let upward_allowance = FOOT_PENETRATION_ALLOWANCE * transform.scale.y;
                        for (slot, leg) in zones.legs.iter().take(foot_count).enumerate() {
                            feet[slot] = probe_foot(
                                leg.foot_joint,
                                world_joints,
                                &model_to_world,
                                &world_to_model,
                                downward_reach,
                                upward_allowance,
                                collision_world,
                                mover_colliders,
                                mover_poses,
                            );
                        }
                    }
                }
            }
        }

        // Read-modify-write: keep the aim/heading fields update_pose_inputs set.
        let mut new_inputs = mesh.pose_inputs.unwrap_or_default();
        new_inputs.feet = feet;
        new_inputs.foot_count = foot_count as u8;
        if mesh.pose_inputs == Some(new_inputs) {
            continue;
        }
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        mesh.pose_inputs = Some(new_inputs);
        let _ = registry.set_component(id, mesh);
    }
}

/// Probe one foot: transform its animated model-space origin to world, cast a
/// bounded downward ray, and convert a walkable hit back to model space for the
/// solver. Returns a miss (`FootProbe::default`) when the foot joint is absent,
/// no ground lies within `reach` below the foot, or the surface is too steep to
/// stand on (the same `COS_WALKABLE` floor threshold movement ground-stick uses).
#[allow(clippy::too_many_arguments)] // a flat parameter list keeps this a leaf helper.
fn probe_foot(
    foot_joint: usize,
    world_joints: &[Mat4],
    model_to_world: &Mat4,
    world_to_model: &Mat4,
    downward_reach: f32,
    upward_allowance: f32,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
) -> FootProbe {
    let miss = FootProbe::default();
    let Some(foot) = world_joints.get(foot_joint) else {
        return miss;
    };
    let foot_world = model_to_world.transform_point3(foot.w_axis.truncate());
    if !foot_world.is_finite() || downward_reach <= 0.0 || upward_allowance < 0.0 {
        return miss;
    }

    let ray_origin = foot_world + Vec3::Y * upward_allowance;
    let max_toi = upward_allowance + downward_reach;
    let origin = Point::new(ray_origin.x, ray_origin.y, ray_origin.z);
    let down = Vector::new(0.0, -1.0, 0.0);
    // Static-only fast path; fold movers in only when present.
    let hit = if mover_colliders.is_empty() {
        cast_ray(collision_world, origin, down, max_toi).map(|h| {
            (
                h.time_of_impact,
                Vec3::new(h.normal.x, h.normal.y, h.normal.z),
            )
        })
    } else {
        cast_ray_combined(
            collision_world,
            mover_colliders,
            mover_poses,
            origin,
            down,
            max_toi,
        )
        .map(|h| (h.time_of_impact, h.normal))
    };
    let Some((toi, normal_world)) = hit else {
        return miss;
    };

    // Walkable-normal convention: ground under the foot must face mostly up.
    if !is_walkable_ground_normal(normal_world) {
        return miss;
    }

    let contact_world = ray_origin + Vec3::new(0.0, -toi, 0.0);
    let contact_model = world_to_model.transform_point3(contact_world);
    // A world normal converted back to model space uses the transpose of the
    // model→world linear map. Using the inverse as a direction is wrong for
    // non-uniform scale.
    let normal_model = model_to_world
        .transpose()
        .transform_vector3(normal_world)
        .normalize_or_zero();
    if !contact_model.is_finite() || normal_model == Vec3::ZERO {
        return miss;
    }
    FootProbe {
        contact_height: contact_model.y,
        normal: normal_model,
        hit: true,
    }
}

fn is_walkable_ground_normal(normal: Vec3) -> bool {
    normal.is_finite() && normal.y >= COS_WALKABLE
}

/// Foot contacts use model-space height, so v1 supports upright entities with
/// positive, invertible scale (uniform or non-uniform). Tilted models would make
/// a world-down ray change model XZ while the current `FootProbe` stores only a
/// height; singular transforms have no valid inverse. Both report fresh misses.
fn foot_probe_inverse(
    transform: &postretro_entities::Transform,
    model_to_world: &Mat4,
) -> Option<Mat4> {
    if !transform.scale.is_finite()
        || transform.scale.min_element() <= 0.0
        || !transform.rotation.is_normalized()
    {
        return None;
    }
    let model_up = transform.rotation * Vec3::Y;
    if model_up.dot(Vec3::Y) < 1.0 - 1.0e-5 {
        return None;
    }
    let inverse = model_to_world.inverse();
    inverse
        .to_cols_array()
        .iter()
        .all(|value| value.is_finite())
        .then_some(inverse)
}

mod host_movement;
mod reload;
mod weapon_stage;

pub(crate) use reload::{ReloadDelivery, ReloadOutcome};
pub(crate) use reload::{
    clear_feedback_for_weapon as clear_reload_feedback_for_weapon,
    clear_owner_feedback_for_weapons as clear_owner_reload_feedback_for_weapons,
};
pub(crate) use weapon_stage::apply_authorized_weapon_impact_damage;

#[cfg(test)]
pub(crate) use host_movement::run_host_movement_tick;

// `pub(crate)` so Task 5/7's timed-reaction test modules (new sibling files) can
// reach `SimHarness` and its `frame`/`new`/`tick`/`record` methods.
#[cfg(test)]
pub(crate) mod determinism_tests;
#[cfg(test)]
mod divergence_spike_tests;
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) mod predict_reconcile;

/// Single-player / single-pawn movement stage. Resolves the local movement pawn via
/// the registry marker, then drives it through the shared host multi-pawn seam
/// (`host_movement::run_host_movement_tick`) with a one-element input list. The host
/// netcode path bypasses this entirely and calls the seam directly with EVERY
/// authoritative pawn — `local_movement_pawn` is the single-player resolver
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
        registry.local_player_movement_pawn()
    };
    let Some(id) = local else {
        return Vec::new();
    };

    let pawn_inputs = [(id, input.clone())];
    let mut registry = registry.borrow_mut();
    host_movement::run_host_movement_tick(&mut registry, collision, gravity, &pawn_inputs, tick_dt)
}
pub(crate) fn run_death_sweep(registry: &Rc<RefCell<EntityRegistry>>) -> Vec<String> {
    let report = {
        let mut registry = registry.borrow_mut();
        scripting_systems::health::sweep_deaths(&mut registry)
    };

    let mut events = Vec::new();
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
    use crate::trigger_system::TriggerEventEdge;
    use crate::weapon::FireButtonState;
    use glam::Vec2;
    use postretro_entities::components::agent::AgentComponent;
    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::components::mesh::{
        AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
        resolve_pending_animation_stamps,
    };
    use postretro_entities::components::touchable::TouchableComponent;
    use postretro_entities::data_descriptors::{
        BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope, MotionVerb,
    };
    use postretro_entities::{
        DescriptorComponentKind, DescriptorMapOverride, DescriptorProvenance, DescriptorSpawnPath,
    };

    /// A direct-graph brain staged directly into its `alert` state.
    fn alert_brain(move_speed: f32) -> BrainComponent {
        let graph = BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::from([
                    (
                        "idle".to_string(),
                        BehaviorActivityDescriptor {
                            animation: Some("idle".to_string()),
                            motion: Some(MotionVerb::Hold),
                            action: None,
                            on_enter: None,
                            layers: BTreeMap::new(),
                        },
                    ),
                    (
                        "alert".to_string(),
                        BehaviorActivityDescriptor {
                            animation: Some("walk".to_string()),
                            motion: Some(MotionVerb::ChaseTarget),
                            action: None,
                            on_enter: None,
                            layers: BTreeMap::new(),
                        },
                    ),
                ]),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed,
        };
        let mut brain = BrainComponent::from_graph(&graph);
        assert!(
            brain.enter_activity_at(
                0,
                brain
                    .graph
                    .envelope
                    .activities
                    .keys()
                    .position(|name| name == "alert")
                    .expect("the graph declares `alert`"),
            )
        );
        brain
    }
    use postretro_entities::{
        AmmoReserve, DataRegistry, KinematicMoverComponent, KinematicMoverMode, MoverCommand,
        NamedReaction, NumericRange, PrimitiveDescriptor, ReactionDescriptor, SlotOwnership,
        SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation,
        TriggerFireMode, TriggerVolumeComponent,
    };
    use postretro_foundation::{
        AirParams, AmmoResource, CapsuleParams, FallParams, FireMode, GroundParams,
        PlayerMovementComponent, PlayerMovementDescriptor, ReloadStyle, ResolutionMode,
        SpeedParams, WeaponDescriptor, WeaponResource,
    };
    use postretro_net::wire::NetworkId;
    use postretro_scripting_core::reaction_dispatch::{
        ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
    };
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn zero_hp_player_remains_present_for_trigger_occupancy_until_despawn() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry
            .set_component(
                pawn,
                HealthComponent {
                    max: 100.0,
                    current: 0.0,
                    hitbox: None,
                    death_handled: true,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        let player = AuthoritativePlayer {
            id: PlayerId::Local(pawn),
            pawn,
        };

        assert!(player_is_present_for_trigger_occupancy(&registry, &player));

        registry.despawn(pawn).unwrap();

        assert!(!player_is_present_for_trigger_occupancy(&registry, &player));
    }

    #[test]
    fn disconnected_remote_fire_context_keeps_trigger_and_occupancy_but_has_no_activator() {
        let trigger = EntityId::from_raw(0x0001_0000);
        let event = TriggerEvent {
            fire: crate::trigger_system::TriggerEventFire {
                trigger,
                player: PlayerId::Remote(77),
                event_name: "left".into(),
            },
            edge: TriggerEventEdge::Exit,
        };

        let context = trigger_fire_context(&event, 3, &BTreeMap::new());
        assert!(context.activator.is_none());
        assert_eq!(context.fired_trigger, Some(trigger));
        assert_eq!(context.occupancy, 3);
    }

    // Regression: the fire context scanned every matching remote command and
    // broadcast one trigger event to duplicate remote pawns.
    #[test]
    fn remote_fire_context_uses_the_collision_canonical_pawn() {
        let trigger = EntityId::from_raw(0x0001_0000);
        let canonical_pawn = EntityId::from_raw(0x0001_0001);
        let duplicate_pawn = EntityId::from_raw(0x0001_0002);
        let event = TriggerEvent {
            fire: crate::trigger_system::TriggerEventFire {
                trigger,
                player: PlayerId::Remote(77),
                event_name: "pressed".into(),
            },
            edge: TriggerEventEdge::Enter,
        };
        let canonical_players = BTreeMap::from([(PlayerId::Remote(77), canonical_pawn)]);

        let context = trigger_fire_context(&event, 1, &canonical_players);
        assert_eq!(context.activator, Some(canonical_pawn));
        assert_ne!(context.activator, Some(duplicate_pawn));
    }

    #[test]
    fn local_fire_context_resolves_the_local_entity_without_player_list_entry() {
        let local_pawn = EntityId::from_raw(0x0001_0001);
        let event = TriggerEvent {
            fire: crate::trigger_system::TriggerEventFire {
                trigger: EntityId::from_raw(0x0001_0000),
                player: PlayerId::Local(local_pawn),
                event_name: "pressed".into(),
            },
            edge: TriggerEventEdge::Enter,
        };

        assert_eq!(
            trigger_fire_context(&event, 1, &BTreeMap::new()).activator,
            Some(local_pawn),
        );
    }

    pub(super) fn weapon_component(credit_source: &str) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: Some(credit_source.to_string()),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        })
    }

    pub(super) fn ammo_weapon_component(
        credit_source: &str,
        capacity: u32,
        reserve: u32,
        reload_ms: u32,
    ) -> (WeaponComponent, AmmoReserve) {
        let descriptor = WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: Some(credit_source.to_string()),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "bullets.light".to_string(),
                magazine: capacity,
                cost_per_shot: 1,
                reserve,
                reload_ms,
                reload_style: ReloadStyle::Magazine,
            })),
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        };
        let descriptor = descriptor.validate().unwrap();
        let mut ammo_reserve = AmmoReserve::new();
        ammo_reserve.credit("bullets.light", reserve);
        (WeaponComponent::from_descriptor(&descriptor), ammo_reserve)
    }

    pub(super) fn zero_movement() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        }
    }

    pub(super) fn sim_command(fire: bool, reload: bool) -> SimCommand {
        SimCommand {
            movement: zero_movement(),
            fire_button: FireButtonState {
                pressed: fire,
                active: fire,
            },
            reload,
            firing_slot: 0,
            select_slot: None,
            use_pressed: false,
            drop_pressed: false,
        }
    }

    pub(super) fn spawn_reload_pair(
        registry: &mut EntityRegistry,
        capacity: u32,
        reserve: u32,
        reload_ms: u32,
        magazine: u32,
    ) -> (EntityId, EntityId) {
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let (mut component, reserve_component) =
            ammo_weapon_component("weapon.test.reload", capacity, reserve, reload_ms);
        component.magazine = magazine;
        registry.set_component(weapon, component).unwrap();
        registry.set_component(pawn, reserve_component).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        (pawn, weapon)
    }

    pub(super) fn trigger_movement() -> PlayerMovementComponent {
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
                    travel_speed: None,
                    clip_index: Some(clip_index),
                },
            );
        }
        MeshComponent::animated(
            "test_model".to_string(),
            MeshAnimation::new(states, "idle".into()),
        )
    }

    #[test]
    fn player_travel_heading_uses_negative_z_as_yaw_zero() {
        let epsilon = 1.0e-5;
        assert!(player_travel_heading_yaw(Vec3::NEG_Z, 9.0).abs() < epsilon);
        assert!(
            (player_travel_heading_yaw(Vec3::X, 9.0) + std::f32::consts::FRAC_PI_2).abs() < epsilon
        );
        assert_eq!(player_travel_heading_yaw(Vec3::ZERO, 0.75), 0.75);
    }

    // Regression: host locomotion hard-coded an `idle` state while connected clients
    // used the descriptor's valid `defaultState`, causing perpetual correction for
    // descriptors whose rest state had another author-defined name.
    #[test]
    fn player_locomotion_uses_descriptor_default_state_as_idle_contract() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, trigger_movement()).unwrap();

        let state = |clip: &str, clip_index| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: Some(clip_index),
        };
        let states = HashMap::from([
            ("stand".to_string(), state("stand", 0)),
            ("walk_forward".to_string(), state("walk", 1)),
        ]);
        let mut animation = MeshAnimation::new(states, "stand".to_string());
        animation.current_state = "walk_forward".to_string();
        registry
            .set_component(
                pawn,
                MeshComponent::animated("player".to_string(), animation),
            )
            .unwrap();

        assert_eq!(
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .velocity,
            Vec3::ZERO,
            "the descriptor default state applies only when the player is stationary"
        );

        update_player_animation_locomotion(&mut registry, &HitZoneStore::new(), 0.0);

        assert_eq!(
            registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "stand"
        );
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
                        per_owner: false,
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
        slots
    }

    pub(super) fn remote_command(
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
            aim_pitch: 0.0,
            command: sim_command(fire, reload),
        }
    }

    pub(super) fn run_remote_only_tick(
        registry: Rc<RefCell<EntityRegistry>>,
        remote: &[RemotePawnCommand],
    ) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
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
            &mut ai_runtime,
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
            |_| {},
        )
    }

    #[test]
    fn touch_runs_without_a_trigger_context_before_ai() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, item) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.set_component(pawn, Inventory::default()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();

            let item = registry.spawn(Transform::default());
            registry
                .set_component(
                    item,
                    TouchableComponent {
                        mode: postretro_entities::TouchMode::Auto,
                        radius: 1.0,
                    },
                )
                .unwrap();
            registry
                .set_component(item, weapon_component("weapon.touch"))
                .unwrap();
            registry
                .set_component(
                    item,
                    DescriptorProvenance {
                        canonical_name: "weapon.touch".to_string(),
                        owned_components: std::collections::BTreeSet::from([
                            DescriptorComponentKind::Weapon,
                            DescriptorComponentKind::Touchable,
                        ]),
                        map_overrides: std::collections::BTreeSet::<DescriptorMapOverride>::new(),
                        spawn_path: DescriptorSpawnPath::MapPlacement,
                    },
                )
                .unwrap();
            registry
                .set_component(item, MeshComponent::stateless("weapon.glb".to_string()))
                .unwrap();
            (pawn, item)
        };
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let mut touch_system = TouchSystem::default();
        let edges = HashMap::new();

        let events = simulate_tick_with_presentation_aim(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            &mut ai_runtime,
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut touch_system,
            &[],
            None,
            &edges,
            &edges,
            None,
            |_| {},
        );

        assert_eq!(
            registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item)
        );
        assert_eq!(events.repointed_pawns, vec![pawn]);
        assert!(
            registry
                .borrow()
                .get_component::<TouchableComponent>(item)
                .is_err()
        );
    }

    #[test]
    fn use_edge_activates_trigger_and_press_item_in_the_same_fixed_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, item, trigger) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.set_component(pawn, Inventory::default()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();

            let item = registry.spawn(Transform::default());
            registry
                .set_component(
                    item,
                    TouchableComponent {
                        mode: postretro_entities::TouchMode::Press,
                        radius: 1.0,
                    },
                )
                .unwrap();
            registry
                .set_component(item, weapon_component("weapon.press"))
                .unwrap();
            registry
                .set_component(
                    item,
                    DescriptorProvenance {
                        canonical_name: "weapon.press".to_string(),
                        owned_components: std::collections::BTreeSet::from([
                            DescriptorComponentKind::Weapon,
                            DescriptorComponentKind::Touchable,
                        ]),
                        map_overrides: std::collections::BTreeSet::<DescriptorMapOverride>::new(),
                        spawn_path: DescriptorSpawnPath::MapPlacement,
                    },
                )
                .unwrap();
            registry
                .set_component(item, MeshComponent::stateless("weapon.glb".to_string()))
                .unwrap();

            let trigger = registry.spawn(Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Use,
                        String::new(),
                        "use_target".to_string(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .unwrap();
            (pawn, item, trigger)
        };
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let mut touch_system = TouchSystem::default();
        let mut trigger_system = TriggerSystem::default();
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(trigger, Vec3::splat(-1.0), Vec3::splat(1.0));
        let bindings = TriggerBindingTable::default();
        let slot_table = Rc::new(RefCell::new(SlotTable::new()));
        let use_edges = HashMap::from([(PlayerId::Local(pawn), true)]);

        let events = simulate_tick_with_presentation_aim(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            &mut ai_runtime,
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut touch_system,
            &[],
            None,
            &use_edges,
            &HashMap::new(),
            Some(TriggerTickContext {
                system: &mut trigger_system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table,
                script_ctx: None,
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
        );

        assert_eq!(
            events.trigger_fires.len(),
            1,
            "the trigger stage consumes the shared Use edge first"
        );
        assert_eq!(events.trigger_fires[0].fire.trigger, trigger);
        assert_eq!(
            registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "the later touch stage reads the same Use edge instead of a consumed one"
        );
    }

    #[test]
    fn unowned_world_weapon_keeps_all_live_state_after_many_fixed_ticks() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let weapon = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.set_component(pawn, Inventory::default()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();

            let weapon = registry.spawn(Transform {
                position: Vec3::new(10.0, 0.0, 0.0),
                ..Transform::default()
            });
            let mut component = weapon_component("weapon.unowned");
            component.state =
                postretro_entities::components::wieldable_state::WieldableState::Reloading;
            component.state_remaining_ms = 800;
            component.state_total_ms = 1_000;
            component.state_elapsed_sub_ms = 0.5;
            component.reload_credited = 1;
            component.cooldown_remaining_ms = 75.0;
            component.shoot_press_consumed = true;
            component.reload_press_consumed = true;
            registry.set_component(weapon, component).unwrap();
            weapon
        };
        let before = registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();

        for _ in 0..5 {
            run_local_only_tick(
                registry.clone(),
                weapon,
                &sim_command(false, false),
                1.0 / 60.0,
            );
        }

        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap(),
            &before,
            "an unowned world weapon is never advanced by the weapon stage"
        );
    }

    pub(super) fn run_local_only_tick(
        registry: Rc<RefCell<EntityRegistry>>,
        weapon: EntityId,
        command: &SimCommand,
        tick_dt: f32,
    ) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        simulate_tick(
            registry,
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
            command,
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            tick_dt,
            None,
            |_| {},
        )
    }

    #[test]
    fn trigger_consequences_run_in_tick_once_and_residual_reaches_app_drain() {
        let script_ctx = postretro_entities::ScriptCtx::new();
        *script_ctx.slot_table.borrow_mut() = trigger_slots();
        script_ctx
            .slot_table
            .borrow_mut()
            .insert(
                "trigger.count".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(NumericRange {
                        min: 0.0,
                        max: 100.0,
                    }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .expect("trigger count slot should be vacant");
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
                        postretro_entities::KinematicMoverConfig {
                            waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                            waypoint_names: vec!["start".into(), "end".into()],
                            speed_mps: 1.0,
                            wait_ms: 0.0,
                            mode: KinematicMoverMode::Once,
                            started: false,
                            spin_axis: Vec3::ZERO,
                            initial_spin_rate_rad_s: 0.0,
                            spin_accel_rad_s2: 0.0,
                            carry_yaw: false,
                        },
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
                        pending_kill_credit: None,
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
                    target: None,
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
                primitive(
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "trigger.count",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "trigger.count" },
                            "b": { "op": "const", "value": 1.0 }
                        }
                    }),
                ),
                primitive(
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "trigger.count",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "trigger.count" },
                            "b": { "op": "const", "value": 1.0 }
                        }
                    }),
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
            TriggerBindingTable::build_with_script_ctx(&registry.borrow(), &data, &script_ctx);
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(source_trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
        let mut trigger_system = TriggerSystem::default();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
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
            &mut ai_runtime,
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
                script_ctx: Some(script_ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
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
                crate::trigger_bindings::BoundTriggerCommandKind::StoreSlot,
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
        let trigger_flag = match script_ctx
            .slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected numeric trigger flag, got {other:?}"),
        };
        assert!(
            (trigger_flag - 1.0).abs() <= 1.0e-5,
            "trigger literal write must set the flag to 1; got {trigger_flag}"
        );
        let trigger_count = match script_ctx
            .slot_table
            .borrow()
            .get("trigger.count")
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected numeric trigger count, got {other:?}"),
        };
        assert!(
            (trigger_count - 2.0).abs() <= 1.0e-5,
            "two IR increments from one on_fire execution must accumulate within the fixed tick; got {trigger_count}"
        );

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);
        let residual = bindings.residual(events.trigger_residuals[0].0).unwrap();
        let _ = fire_prepartitioned_reactions_with_sequences(
            residual.steps(),
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
            ResidualOrigin::TriggerBinding,
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
            &mut ai_runtime,
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
                script_ctx: Some(script_ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
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
                            pending_kill_credit: None,
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
                            target: None,
                            tag: Some("second-trigger".into()),
                            on_complete: None,
                            args: serde_json::json!({}),
                        }),
                    },
                    NamedReaction {
                        name: "second".into(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: "applyDamage".into(),
                            target: None,
                            tag: Some("damage-target".into()),
                            on_complete: None,
                            args: serde_json::json!({ "amount": 5 }),
                        }),
                    },
                ],
                Vec::new(),
                &[],
            );
            let bindings =
                TriggerBindingTable::build_with_script_ctx(&registry.borrow(), &data, &script_ctx);
            let mut bridge = TriggerVolumeBridge::new();
            for trigger in [first, second] {
                bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
            }
            let mut trigger_system = TriggerSystem::default();
            let mut progress = ProgressTracker::new();
            let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
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
                &mut ai_runtime,
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
                    script_ctx: Some(script_ctx.clone()),
                    auto_close_timers: None,
                    use_edges: &use_edges,
                }),
                |_| {},
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
    fn synchronous_producer_queue_waits_until_next_tick_before_first_decrement() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = {
            let mut registry = registry.borrow_mut();
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
            target
        };
        let run_tick = |enqueue_effect: bool| {
            let world = CollisionWorld::new();
            let hit_zones = HitZoneStore::new();
            let mut progress = ProgressTracker::new();
            let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
            let mut mover_states = MoverTickStateTable::default();
            simulate_tick(
                registry.clone(),
                &world,
                &hit_zones,
                None,
                -9.81,
                None,
                0.0,
                &mut progress,
                &mut ai_runtime,
                &[],
                &mut mover_states,
                &[],
                &sim_command(false, false),
                |registry| {
                    if enqueue_effect {
                        crate::impact_effects::set_health(
                            &mut registry.borrow_mut(),
                            target,
                            25.0,
                            Some(100.0),
                        );
                    }
                    PostMovementCommand {
                        aim_origin: Vec3::ZERO,
                        aim_direction: Vec3::NEG_Z,
                    }
                },
                0.040,
                None,
                |_| {},
            );
        };

        run_tick(true);
        assert_eq!(
            registry
                .borrow()
                .get_component::<postretro_entities::DeferredEffectComponent>(target)
                .unwrap()
                .pending[0]
                .remaining_us,
            100_000,
        );

        run_tick(false);
        assert_eq!(
            registry
                .borrow()
                .get_component::<postretro_entities::DeferredEffectComponent>(target)
                .unwrap()
                .pending[0]
                .remaining_us,
            60_000,
        );
    }

    // Regression: the AI brain skipped a queued despawn, but the later
    // fixed-tick steering stage still followed its retained path.
    #[test]
    fn fixed_tick_queued_despawn_quiesces_brain_agent_steering() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (enemy, start) = {
            let mut registry = registry.borrow_mut();
            let start = Transform {
                position: Vec3::new(2.0, 1.0, 2.0),
                ..Transform::default()
            };
            let enemy = registry.spawn(start);

            let destination = Vec3::new(12.0, 1.0, 2.0);
            let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
            agent.path = vec![destination];
            agent.mandatory_waypoints = vec![false];
            agent.destination = Some(destination);
            agent.planned_destination = Some(destination);
            agent.replan_cooldown_ticks = 10;
            registry.set_component(enemy, agent).unwrap();
            registry.set_component(enemy, alert_brain(4.0)).unwrap();
            crate::impact_effects::despawn(&mut registry, enemy, Some(500.0));
            (enemy, start)
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
            0.0,
            None,
            0.0,
            &mut progress,
            &mut ai_runtime,
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            0.1,
            None,
            |_| {},
        );

        assert!(events.ai.is_empty(), "queued despawn must suppress attacks");
        let registry = registry.borrow();
        assert_eq!(
            registry.get_component::<Transform>(enemy).unwrap().position,
            start.position,
            "the real fixed-tick steering stage must not move a queued-despawn brain",
        );
        let agent = registry.get_component::<AgentComponent>(enemy).unwrap();
        assert_eq!(agent.velocity, Vec3::ZERO);
        assert_eq!(agent.destination, Some(Vec3::new(12.0, 1.0, 2.0)));
        let effects = registry
            .get_component::<postretro_entities::DeferredEffectComponent>(enemy)
            .unwrap();
        assert!(
            !effects.inert,
            "the delayed removal countdown is still active"
        );
        assert!(
            effects.pending[0].remaining_us.abs_diff(400_000) <= 1,
            "one 0.1s fixed tick leaves about 400ms; got {}us",
            effects.pending[0].remaining_us,
        );
    }

    #[test]
    fn authorized_remote_hit_latches_without_reporting_or_removal_in_same_host_tick() {
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
                        pending_kill_credit: None,
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

        let death_events = run_death_sweep(&registry);

        assert!(death_events.is_empty());
        assert!(
            registry.borrow().exists(target),
            "the post-HIT sweep leaves a zero-HP target live for authored despawn"
        );
        let health = registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        assert!(health.death_handled);
        assert!(
            health.pending_kill_credit.is_some(),
            "the sweep freezes credit but emits no progress event",
        );
    }
}
