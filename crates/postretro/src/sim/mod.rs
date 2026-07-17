// Headless fixed-tick game-state advance seam.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

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
use crate::trigger_bindings::{TriggerBindingTable, TriggerResidualHandle};
use crate::trigger_commands::TriggerFireContext;
#[cfg(test)]
use crate::trigger_system::TriggerEvent;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};
use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
use postretro_entities::PoseInputs;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::{BrainComponent, LogicalState};
use postretro_entities::components::health::{
    DamageContext, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::mesh::{MeshAnimation, MeshComponent};
use postretro_entities::components::weapon::{UNKNOWN_WEAPON_CREDIT_SOURCE, WeaponComponent};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, ScriptCtx, SlotTable};
use postretro_foundation::pose::{FootProbe, MAX_FEET};
use postretro_model::ModelHandle;
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
    /// Present for production level installs and IR-aware harnesses. Literal
    /// fixtures may omit it and keep their direct table execution path.
    pub(crate) script_ctx: Option<ScriptCtx>,
    pub(crate) use_edges: &'a HashMap<PlayerId, bool>,
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
            registry.local_player_movement_pawn()
        };
        if let Some(pawn) = local_player {
            let id = PlayerId::Local(pawn);
            players.push(AuthoritativePlayer { id, pawn });
        }
        let canonical_player_pawns = {
            let registry = registry.borrow();
            crate::trigger_system::canonical_player_pawns(&registry, &players)
        };
        let alive_players: HashSet<PlayerId> = {
            let registry = registry.borrow();
            players
                .iter()
                .filter(|player| {
                    registry
                        .get_component::<HealthComponent>(player.pawn)
                        .map_or(true, |health| health.current > 0.0)
                })
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
                        trigger_residuals.push(handle);
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
                        trigger_residuals.push(handle);
                    }
                },
            );
        }
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
        update_brain_animation_playback_rates(&mut registry, hit_zone_store, anim_time);
        update_pose_inputs(&mut registry);
        update_foot_ground_probes(
            &mut registry,
            collision_world,
            mover_colliders,
            &*mover_tick_states,
            hit_zone_store,
            anim_time,
        );
    }

    let (authorized_shots, mut reload_deliveries, remote_weapon_events) =
        run_remote_weapon_commands(&registry, remote_pawn_commands, tick_dt);
    let own_pawn = {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    let weapon_fire = weapon_fire_command(command.fire_button, post_movement_command);
    let (local_deliveries, mut weapon) = run_local_weapon_command(
        &registry,
        own_pawn,
        active_wieldable,
        &weapon_fire,
        command.reload,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    reload_deliveries.extend(local_deliveries);
    weapon.extend(remote_weapon_events);
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

        // Rate-scale only the alert-mapped locomotion state, and only while the
        // archetype leaves `speedScale` on. Every other case rests at the
        // authored rate (1.0). Calibration is `measured_ground_speed /
        // effective_travel_speed`; a state with neither an override nor a derived
        // clip stride falls back to `speed_xz / move_speed`, keeping the shipped
        // in-place walk unchanged.
        let is_locomotion =
            animation.current_state == brain.tuning.states.animation_for(LogicalState::Alert);
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
/// model handle and the active state's resolved clip index). `None` when neither
/// is calibrated — the degenerate reference (`speed_xz / move_speed`) that keeps
/// the shipped in-place walk byte-for-byte unchanged, since Task 2 derives no
/// travel speed for an in-place clip.
fn effective_travel_speed(
    animation: &MeshAnimation,
    mesh: &MeshComponent,
    hit_zone_store: &HitZoneStore,
) -> Option<f32> {
    let state = animation.states.get(&animation.current_state)?;
    if let Some(override_speed) = state.travel_speed {
        return Some(override_speed);
    }
    let clip_index = state.clip_index?;
    hit_zone_store
        .get(&ModelHandle::from(mesh.model.clone()))?
        .clips
        .get(clip_index)?
        .travel_speed
}

/// Write same-tick presentation inputs after AI and steering have settled the
/// entity's target and body rotation. Every animated mesh receives a finite
/// value; entities without a live acquired target hold their body heading with
/// zero pitch, making pose modifiers a visual no-op.
fn update_pose_inputs(registry: &mut EntityRegistry) {
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

        let (aim_pitch, aim_yaw) = target_position
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

/// Model-space downward reach of each foot ground probe, in model units. Ground
/// farther than this below the animated foot reads as no contact — a swing foot
/// with no plantable surface — sized for the roughly unit-tall models the loader
/// ships. Scaled by the entity's model scale at cast time so the bound stays
/// constant in model space regardless of instance scale.
const FOOT_PLANTING_REACH: f32 = 0.5;

/// Sample each leg-tagged animated entity's UNMODIFIED world foot pose, cast a
/// short downward ray at the collision world under each foot, and write the
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
    // Leg-tagged animated meshes only. Collect ids first so the per-entity
    // sample/cast can borrow the registry without aliasing the write-back.
    let legged: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, _)| {
            let mesh = registry.get_component::<MeshComponent>(id).ok()?;
            if mesh.animation.is_none() {
                return None;
            }
            let zones = hit_zone_store.get(&ModelHandle::from(mesh.model.clone()))?;
            (!zones.legs.is_empty()).then_some(id)
        })
        .collect();

    for id in legged {
        let Ok(mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let Some(zones) = hit_zone_store.get(&ModelHandle::from(mesh.model.clone())) else {
            continue;
        };
        let Ok(transform) = registry
            .get_component::<postretro_entities::Transform>(id)
            .copied()
        else {
            continue;
        };
        // Same model→world placement capsules and the renderer use, so the probe
        // casts from where the foot is actually drawn.
        let Some(model_to_world) = model_matrix(&transform, mesh.origin_offset) else {
            continue;
        };
        let world_to_model = model_to_world.inverse();
        let reach = FOOT_PLANTING_REACH * max_abs_component(transform.scale);

        // Unmodified world-joint pose (model space); `None` when the precise pose
        // is unavailable — every foot then reports a miss this tick.
        let world_joints =
            sample_world_pose_for_probe(zones, mesh.animation.as_ref(), anim_time, id.to_raw());

        let foot_count = zones.legs.len().min(MAX_FEET);
        let mut feet = [FootProbe::default(); MAX_FEET];
        if let Some(world_joints) = world_joints.as_ref() {
            for (slot, leg) in zones.legs.iter().take(foot_count).enumerate() {
                feet[slot] = probe_foot(
                    leg.foot_joint,
                    world_joints,
                    &model_to_world,
                    &world_to_model,
                    reach,
                    collision_world,
                    mover_colliders,
                    mover_poses,
                );
            }
        }

        // Read-modify-write: keep the aim/heading fields update_pose_inputs set.
        let mut new_inputs = mesh.pose_inputs.unwrap_or_default();
        new_inputs.feet = feet;
        new_inputs.foot_count = foot_count as u8;
        if mesh.pose_inputs == Some(new_inputs) {
            continue;
        }
        let mut mesh = mesh;
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
    reach: f32,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
) -> FootProbe {
    let miss = FootProbe::default();
    let Some(foot) = world_joints.get(foot_joint) else {
        return miss;
    };
    let foot_world = model_to_world.transform_point3(foot.w_axis.truncate());
    if !foot_world.is_finite() || reach <= 0.0 {
        return miss;
    }

    let origin = Point::new(foot_world.x, foot_world.y, foot_world.z);
    let down = Vector::new(0.0, -1.0, 0.0);
    // Static-only fast path; fold movers in only when present.
    let hit = if mover_colliders.is_empty() {
        cast_ray(collision_world, origin, down, reach).map(|h| {
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
            reach,
        )
        .map(|h| (h.time_of_impact, h.normal))
    };
    let Some((toi, normal_world)) = hit else {
        return miss;
    };

    // Walkable-normal convention: ground under the foot must face mostly up.
    if !normal_world.is_finite() || normal_world.y < COS_WALKABLE {
        return miss;
    }

    let contact_world = foot_world + Vec3::new(0.0, -toi, 0.0);
    let contact_model = world_to_model.transform_point3(contact_world);
    let normal_model = world_to_model
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

fn max_abs_component(v: Vec3) -> f32 {
    v.x.abs().max(v.y.abs()).max(v.z.abs())
}

mod host_movement;
mod reload;

pub(crate) use reload::{ReloadDelivery, ReloadOutcome};
pub(crate) use reload::{
    clear_all_feedback as clear_all_reload_feedback,
    clear_feedback_for_weapon as clear_reload_feedback_for_weapon,
};

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
) -> (
    Vec<OpenAuthorizedShot>,
    Vec<ReloadDelivery>,
    Vec<&'static str>,
) {
    let mut registry = registry.borrow_mut();
    let mut authorized = Vec::new();
    let mut reload_deliveries = Vec::new();
    let mut weapon_events = Vec::new();

    for remote in remote_pawn_commands {
        let Some(weapon) = remote.weapon else {
            continue;
        };
        let Ok(mut weapon_component) = registry.get_component::<WeaponComponent>(weapon).cloned()
        else {
            continue;
        };
        let deliveries = reload::tick(
            &mut registry,
            remote.pawn,
            weapon,
            &mut weapon_component,
            remote.command.reload,
            tick_dt,
        );
        let reload_started = deliveries
            .iter()
            .any(|delivery| delivery.outcome == ReloadOutcome::Started);
        reload_deliveries.extend(deliveries);

        let command = WeaponFireCommand {
            button: remote.command.fire_button,
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::Z,
            // Repurposes `can_fire` (elsewhere "aim valid") to mean "pawn has a NetworkId";
            // the real fire gate is `button` -> `wants_fire`. The host casts no local aim ray.
            can_fire: remote.shot_id.is_some(),
        };
        let result = weapon::tick_state_only_component(
            &mut weapon_component,
            &command,
            tick_dt,
            reload_started,
        );
        let effective = weapon_component.effective();
        let damage = effective.damage;
        let range = effective.range;
        let credit_source = effective.credit_source.to_string();
        let _ = registry.set_component(weapon, weapon_component);
        match result {
            WeaponFireAuthorization::Accepted => {}
            WeaponFireAuthorization::Empty => {
                weapon_events.push("dry_fire");
                continue;
            }
            WeaponFireAuthorization::Rejected => continue,
        }
        let Some(shot_id) = remote.shot_id else {
            continue;
        };
        authorized.push(OpenAuthorizedShot {
            shot: AuthorizedShot {
                shot_id,
                pawn: remote.pawn,
                weapon,
                fire_tick: remote.fire_tick,
                damage,
                range,
                pellet_count: 1,
                credit_source,
            },
            owner_client_id: remote.owner_client_id,
        });
    }

    (authorized, reload_deliveries, weapon_events)
}

#[allow(clippy::too_many_arguments)]
fn run_local_weapon_command(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    active_wieldable: Option<EntityId>,
    command: &WeaponFireCommand,
    reload_pressed: bool,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
) -> (Vec<ReloadDelivery>, Vec<&'static str>) {
    let Some(weapon_id) = active_wieldable else {
        return (Vec::new(), Vec::new());
    };
    let mut registry = registry.borrow_mut();
    let Ok(mut weapon_component) = registry
        .get_component::<WeaponComponent>(weapon_id)
        .cloned()
    else {
        return (Vec::new(), Vec::new());
    };
    let deliveries = pawn.map_or_else(Vec::new, |pawn| {
        reload::tick(
            &mut registry,
            pawn,
            weapon_id,
            &mut weapon_component,
            reload_pressed,
            tick_dt,
        )
    });
    let reload_started = deliveries
        .iter()
        .any(|delivery| delivery.outcome == ReloadOutcome::Started);
    let events = weapon::tick_resolved_component(
        &registry,
        &mut weapon_component,
        command,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        reload_started,
    );
    let _ = registry.set_component(weapon_id, weapon_component);
    if let Some(impact) = events.impact.as_ref() {
        weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);
        let attacker = pawn;
        apply_weapon_impact_damage(&mut registry, active_wieldable, attacker, impact);
    }
    (deliveries, events.event_names())
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
        effective.credit_source.to_string(),
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
fn deliver_reload_to_weapon(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon: EntityId,
    reload_pressed: bool,
    tick_dt: f32,
) -> Vec<ReloadDelivery> {
    let Ok(mut component) = registry.get_component::<WeaponComponent>(weapon).cloned() else {
        return Vec::new();
    };
    let deliveries = reload::tick(
        registry,
        pawn,
        weapon,
        &mut component,
        reload_pressed,
        tick_dt,
    );
    let _ = registry.set_component(weapon, component);
    deliveries
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
    use postretro_entities::components::mesh::{
        AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
        resolve_pending_animation_stamps,
    };
    use postretro_entities::{
        AmmoReserve, DataRegistry, KinematicMoverComponent, KinematicMoverMode, MoverCommand,
        NamedReaction, NumericRange, PrimitiveDescriptor, ReactionDescriptor, SlotOwnership,
        SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation,
        TriggerFireMode, TriggerVolumeComponent,
    };
    use postretro_foundation::{
        AirParams, AmmoResource, CapsuleParams, FallParams, FireMode, GroundParams,
        PlayerMovementComponent, PlayerMovementDescriptor, ResolutionMode, SpeedParams,
        WeaponDescriptor, WeaponResource,
    };
    use postretro_net::wire::NetworkId;
    use postretro_scripting_core::reaction_dispatch::fire_prepartitioned_reactions_with_sequences;
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use std::collections::{BTreeMap, HashMap, HashSet};

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

    fn weapon_component(credit_source: &str) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: Some(credit_source.to_string()),
            resource: None,
        })
    }

    fn ammo_weapon_component(
        credit_source: &str,
        capacity: u32,
        reserve: u32,
        reload_ms: u32,
    ) -> (WeaponComponent, AmmoReserve) {
        let descriptor = WeaponDescriptor {
            damage: 10.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: Some(credit_source.to_string()),
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "bullets.light".to_string(),
                magazine: capacity,
                cost_per_shot: 1,
                reserve,
                reload_ms,
            })),
        };
        let descriptor = descriptor.validate().unwrap();
        let mut ammo_reserve = AmmoReserve::new();
        ammo_reserve.credit("bullets.light", reserve);
        (WeaponComponent::from_descriptor(&descriptor), ammo_reserve)
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

    fn spawn_reload_pair(
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
        (pawn, weapon)
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
                        accumulate: None,
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

    fn run_local_only_tick(
        registry: Rc<RefCell<EntityRegistry>>,
        weapon: EntityId,
        command: &SimCommand,
        tick_dt: f32,
    ) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_warned = HashSet::new();
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
            &mut ai_warned,
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
                script_ctx: Some(script_ctx.clone()),
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
                script_ctx: Some(script_ctx.clone()),
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
                    script_ctx: Some(script_ctx.clone()),
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
        assert_eq!(started.reload_remaining_ms, 750);
        assert_eq!(started.reload_total_ms, 1000);
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
                .reload_remaining_ms,
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
        assert_eq!(completed.reload_remaining_ms, 0);
        assert_eq!(completed.reload_total_ms, 1000);
        assert_eq!(completed.cooldown_remaining_ms, 123.0);
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
        assert_eq!(component.reload_total_ms, 100);
        assert_eq!(component.reload_remaining_ms, 0);
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
                .reload_remaining_ms
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
                .reload_remaining_ms,
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
                .reload_remaining_ms,
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
        assert_eq!(full.reload_remaining_ms, 0);
        assert_eq!(full.reload_total_ms, 0);
        assert_eq!(full.magazine, 10);

        let (empty_pawn, empty_weapon) = spawn_reload_pair(&mut registry, 10, 0, 900, 2);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, empty_pawn, empty_weapon, true, 0.1)[0].outcome,
            ReloadOutcome::BlockedEmpty
        );
        let empty = registry
            .get_component::<WeaponComponent>(empty_weapon)
            .unwrap();
        assert_eq!(empty.reload_remaining_ms, 0);
        assert_eq!(empty.reload_total_ms, 0);
        assert_eq!(empty.magazine, 2);
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
        assert_eq!(component.reload_remaining_ms, 700);
        assert_eq!(component.reload_total_ms, 1000);
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
        let mut ai_warned = HashSet::new();
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
            &mut ai_warned,
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
        assert_eq!(weapon.reload_remaining_ms, 750);
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
        assert_eq!(component.reload_remaining_ms, 0);
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
                .reload_remaining_ms,
            0
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
