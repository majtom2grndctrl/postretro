//! Fixed-tick deterministic kinematic mover driver.
//!
//! The driver is intentionally a pure function of component phase, static path
//! data, and fixed `dt`. It reads no clock, RNG, host role, or external state.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use glam::Vec3;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverMode, MoverCommand, Transform,
};
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};
use postretro_scripting_core::sequence::{SequenceError, SequencedPrimitiveRegistry};
use serde::Deserialize;

use crate::collision::moving::{MoverPose, MoverPoseSource};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoverTickState {
    pub(crate) entity: EntityId,
    pub(crate) transform: Transform,
    pub(crate) linear_velocity: Vec3,
    pub(crate) tick_delta: Vec3,
    pub(crate) tick_dt: f32,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MoverTickStateTable {
    states: HashMap<u32, MoverTickState>,
}

impl MoverTickStateTable {
    pub(crate) fn clear(&mut self) {
        self.states.clear();
    }

    pub(crate) fn publish(&mut self, mover_id: u32, state: MoverTickState) {
        self.states.insert(mover_id, state);
    }

    pub(crate) fn get(&self, mover_id: u32) -> Option<&MoverTickState> {
        self.states.get(&mover_id)
    }
}

impl MoverPoseSource for MoverTickStateTable {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.get(mover_id).map(|state| MoverPose {
            transform: state.transform,
            linear_velocity: state.linear_velocity,
            tick_delta: state.tick_delta,
            tick_dt: state.tick_dt,
        })
    }
}

/// Run every active `KinematicMover` component once and republish the live
/// side-table. Callers wire this after `snapshot_transforms` and before any
/// collision consumer for the tick.
pub(crate) fn run_kinematic_mover_tick(
    registry: &mut EntityRegistry,
    side_table: &mut MoverTickStateTable,
    tick_dt: f32,
) {
    side_table.clear();

    let snapshots: Vec<(EntityId, KinematicMoverComponent, Transform)> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(id, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            let transform = *registry.get_component::<Transform>(id).ok()?;
            Some((id, mover.clone(), transform))
        })
        .collect();

    for (entity, mut mover, mut transform) in snapshots {
        let pose = advance_mover_phase_one_tick(&mut mover, &mut transform, tick_dt);

        let _ = registry.set_component(entity, mover.clone());
        let _ = registry.set_component(entity, transform);
        side_table.publish(
            mover.mover_id,
            MoverTickState {
                entity,
                transform,
                linear_velocity: pose.linear_velocity,
                tick_delta: pose.tick_delta,
                tick_dt: pose.tick_dt,
            },
        );
    }
}

pub(crate) fn mover_pose_for_current_phase(
    transform: Transform,
    mover: &KinematicMoverComponent,
    tick_dt: f32,
) -> MoverPose {
    let mut transform = transform;
    transform.position = position_for_phase(mover);
    let linear_velocity = mover.current_linear_velocity;
    MoverPose {
        transform,
        linear_velocity,
        tick_delta: if tick_dt.is_finite() && tick_dt > 0.0 {
            linear_velocity * tick_dt
        } else {
            Vec3::ZERO
        },
        tick_dt,
    }
}

pub(crate) fn advance_mover_phase_one_tick(
    mover: &mut KinematicMoverComponent,
    transform: &mut Transform,
    tick_dt: f32,
) -> MoverPose {
    let start_position = position_for_phase(mover);
    transform.position = start_position;
    let end_position = advance_mover(mover, tick_dt);
    transform.position = end_position;
    let tick_delta = end_position - start_position;
    let linear_velocity = if tick_dt.is_finite() && tick_dt > 0.0 {
        tick_delta / tick_dt
    } else {
        Vec3::ZERO
    };
    mover.current_linear_velocity = linear_velocity;
    MoverPose {
        transform: *transform,
        linear_velocity,
        tick_delta,
        tick_dt,
    }
}

/// Apply a declarative command by mutating only deterministic mover phase.
///
/// This deliberately has no registry, clock, RNG, or host-role dependency so
/// the same command produces the same next phase for every simulation peer.
pub(crate) fn apply_mover_command(mover: &mut KinematicMoverComponent, command: &MoverCommand) {
    match command {
        MoverCommand::Start => {
            if mover.completed || (mover.started && mover.wait_remaining_ms <= 0.0) {
                return;
            }
            mover.started = true;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::Stop => {
            if !mover.started {
                return;
            }
            mover.started = false;
            mover.current_linear_velocity = Vec3::ZERO;
        }
        MoverCommand::Reverse => {
            reanchor_direction(mover, if mover.direction_sign >= 0 { -1 } else { 1 });
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::GoToPathNode(name) => {
            let mut matches = mover
                .waypoint_names
                .iter()
                .enumerate()
                .filter_map(|(index, waypoint_name)| (waypoint_name == name).then_some(index));
            let Some(target) = matches.next() else {
                log::warn!(
                    "[Mover] go_to_path_node for mover {} references unknown waypoint `{name}`; skipping",
                    mover.mover_id
                );
                return;
            };
            if matches.next().is_some() || target > usize::from(u16::MAX) {
                log::warn!(
                    "[Mover] go_to_path_node for mover {} cannot uniquely resolve waypoint `{name}`; skipping",
                    mover.mover_id
                );
                return;
            }
            let target = target as u16;
            if mover_is_at_waypoint(mover, target) {
                return;
            }

            let direction = if path_coordinate(mover)
                .map(|coordinate| f32::from(target) > coordinate)
                .unwrap_or(target > mover.segment_index)
            {
                1
            } else {
                -1
            };
            reanchor_direction(mover, direction);
            mover.target_segment = Some(target);
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
        }
    }
}

/// Apply one command to an already-resolved tag target set. Non-movers remain
/// untouched so a mixed tag cannot accidentally gain a mover component.
pub(crate) fn apply_mover_command_to_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    command: &MoverCommand,
) {
    for &entity in targets {
        let Ok(mut mover) = registry
            .get_component::<KinematicMoverComponent>(entity)
            .cloned()
        else {
            warn_non_mover_target_once(entity);
            continue;
        };
        apply_mover_command(&mut mover, command);
        let _ = registry.set_component(entity, mover);
    }
}

/// Register the closed mover command vocabulary for named, tag-targeted
/// reactions. Each route intentionally converges on the shared command applier
/// used by KVP trigger dispatch.
pub(crate) fn register_mover_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("moverStart", |registry, targets, _args| {
        apply_mover_command_to_targets(registry, targets, &MoverCommand::Start);
        Ok(())
    });
    registry.register("moverStop", |registry, targets, _args| {
        apply_mover_command_to_targets(registry, targets, &MoverCommand::Stop);
        Ok(())
    });
    registry.register("moverReverse", |registry, targets, _args| {
        apply_mover_command_to_targets(registry, targets, &MoverCommand::Reverse);
        Ok(())
    });
    registry.register("moverGoToPathNode", |registry, targets, args| {
        let args: MoverGoToPathNodeArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("moverGoToPathNode: failed to deserialize args: {e}"),
            })?;
        apply_mover_command_to_targets(registry, targets, &MoverCommand::GoToPathNode(args.node));
        Ok(())
    });
}

/// Register the same command vocabulary on the per-entity sequence path.
/// SDK mover handles return sequence-step arrays, while direct primitive
/// reactions use the tag-targeted registry above.
pub(crate) fn register_sequenced_mover_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: postretro_entities::ScriptCtx,
) {
    register_sequenced_mover_command(registry, ctx.clone(), "moverStart", MoverCommand::Start);
    register_sequenced_mover_command(registry, ctx.clone(), "moverStop", MoverCommand::Stop);
    register_sequenced_mover_command(registry, ctx.clone(), "moverReverse", MoverCommand::Reverse);
    registry.register("moverGoToPathNode", move |id, args| {
        let args: MoverGoToPathNodeArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("moverGoToPathNode: failed to deserialize args: {e}"),
            })?;
        let mut entities = ctx.registry.borrow_mut();
        apply_mover_command_to_targets(
            &mut entities,
            &[id],
            &MoverCommand::GoToPathNode(args.node),
        );
        Ok(())
    });
}

fn register_sequenced_mover_command(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: postretro_entities::ScriptCtx,
    name: &'static str,
    command: MoverCommand,
) {
    registry.register(name, move |id, _args| {
        let mut entities = ctx.registry.borrow_mut();
        apply_mover_command_to_targets(&mut entities, &[id], &command);
        Ok(())
    });
}

#[derive(Debug, Deserialize)]
struct MoverGoToPathNodeArgs {
    node: String,
}

fn warn_non_mover_target_once(entity: EntityId) {
    static WARNED_TARGETS: OnceLock<Mutex<HashSet<EntityId>>> = OnceLock::new();
    let warned = WARNED_TARGETS.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut warned) = warned.lock() else {
        return;
    };
    if warned.insert(entity) {
        log::warn!("[Mover] command target {entity} has no KinematicMoverComponent; skipping");
    }
}

fn advance_mover(mover: &mut KinematicMoverComponent, tick_dt: f32) -> Vec3 {
    let mut position = position_for_phase(mover);
    let mut remaining_ms = if tick_dt.is_finite() && tick_dt > 0.0 {
        tick_dt * 1000.0
    } else {
        0.0
    };

    if remaining_ms <= 0.0 || !mover.started || mover.completed || !path_can_move(mover) {
        return position;
    }

    while remaining_ms > 0.0 {
        if mover.wait_remaining_ms > 0.0 {
            let consumed = mover.wait_remaining_ms.min(remaining_ms);
            mover.wait_remaining_ms -= consumed;
            remaining_ms -= consumed;
            position = endpoint_position(mover);
            if mover.wait_remaining_ms > 0.0 {
                break;
            }
            if mover.target_segment == Some(mover.segment_index) {
                mover.target_segment = None;
                mover.completed = true;
                break;
            }
            if mover.mode == KinematicMoverMode::Once && at_final_endpoint(mover) {
                mover.completed = true;
                break;
            }
            continue;
        }

        let Some((from_index, to_index)) = segment_indices(mover) else {
            if mover.mode == KinematicMoverMode::Once && at_final_endpoint(mover) {
                mover.completed = true;
            }
            position = endpoint_position(mover);
            break;
        };

        let from = mover.waypoints[from_index];
        let to = mover.waypoints[to_index];
        let segment = to - from;
        let length = segment.length();
        if !length.is_finite() || length <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH {
            mover.segment_elapsed_ms = 0.0;
            mover.segment_index = to_index as u16;
            position = to;
            mover.completed = true;
            break;
        }

        let duration_ms = (length / mover.speed_mps) * 1000.0;
        let elapsed = mover.segment_elapsed_ms.clamp(0.0, duration_ms);
        let available_ms = (duration_ms - elapsed).max(0.0);
        if remaining_ms < available_ms {
            mover.segment_elapsed_ms = elapsed + remaining_ms;
            let fraction = mover.segment_elapsed_ms / duration_ms;
            position = from.lerp(to, fraction);
            remaining_ms = 0.0;
        } else {
            remaining_ms -= available_ms;
            mover.segment_elapsed_ms = 0.0;
            mover.segment_index = to_index as u16;
            position = to;
            handle_arrival_at_waypoint(mover);
            if mover.completed {
                break;
            }
        }
    }

    position
}

fn path_can_move(mover: &KinematicMoverComponent) -> bool {
    mover.waypoints.len() >= 2 && mover.speed_mps.is_finite() && mover.speed_mps > 0.0
}

fn segment_indices(mover: &KinematicMoverComponent) -> Option<(usize, usize)> {
    let from = usize::from(mover.segment_index);
    let to = if mover.direction_sign >= 0 {
        from.checked_add(1)?
    } else {
        from.checked_sub(1)?
    };
    (from < mover.waypoints.len() && to < mover.waypoints.len()).then_some((from, to))
}

fn handle_arrival_at_waypoint(mover: &mut KinematicMoverComponent) {
    let last = mover.waypoints.len().saturating_sub(1);
    let current = usize::from(mover.segment_index);
    if mover.target_segment == Some(mover.segment_index) {
        if mover.wait_ms.is_finite() && mover.wait_ms > 0.0 {
            mover.wait_remaining_ms = mover.wait_ms;
        } else {
            mover.target_segment = None;
            mover.completed = true;
        }
        return;
    }
    match mover.mode {
        KinematicMoverMode::Once if current == last => {
            if mover.wait_ms.is_finite() && mover.wait_ms > 0.0 {
                mover.wait_remaining_ms = mover.wait_ms;
            } else {
                mover.completed = true;
            }
        }
        KinematicMoverMode::PingPong if current == 0 || current == last => {
            mover.direction_sign = if current == 0 { 1 } else { -1 };
            if mover.wait_ms.is_finite() && mover.wait_ms > 0.0 {
                mover.wait_remaining_ms = mover.wait_ms;
            }
        }
        _ => {}
    }
}

fn position_for_phase(mover: &KinematicMoverComponent) -> Vec3 {
    if mover.waypoints.is_empty() {
        return Vec3::ZERO;
    }
    if mover.completed {
        return endpoint_position(mover);
    }
    if mover.wait_remaining_ms > 0.0 {
        return endpoint_position(mover);
    }
    let Some((from_index, to_index)) = segment_indices(mover) else {
        return endpoint_position(mover);
    };
    let from = mover.waypoints[from_index];
    let to = mover.waypoints[to_index];
    let length = (to - from).length();
    if !length.is_finite()
        || length <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH
        || mover.speed_mps <= 0.0
    {
        return from;
    }
    let duration_ms = (length / mover.speed_mps) * 1000.0;
    let fraction = (mover.segment_elapsed_ms / duration_ms).clamp(0.0, 1.0);
    from.lerp(to, fraction)
}

fn endpoint_position(mover: &KinematicMoverComponent) -> Vec3 {
    let index = usize::from(mover.segment_index).min(mover.waypoints.len().saturating_sub(1));
    mover.waypoints.get(index).copied().unwrap_or(Vec3::ZERO)
}

fn at_final_endpoint(mover: &KinematicMoverComponent) -> bool {
    usize::from(mover.segment_index) == mover.waypoints.len().saturating_sub(1)
}

fn mover_is_at_waypoint(mover: &KinematicMoverComponent, target: u16) -> bool {
    mover.segment_index == target && mover.segment_elapsed_ms <= f32::EPSILON
}

/// Re-express the current pose using a chosen traversal direction. The mover
/// stores the *from* waypoint as `segment_index`, so a reversal must switch
/// that cursor to the old destination as well as complement elapsed time.
fn reanchor_direction(mover: &mut KinematicMoverComponent, direction: i8) {
    let direction = if direction >= 0 { 1 } else { -1 };
    let Some((from, to)) = segment_indices(mover) else {
        // Reversing at an endpoint can request the direction that points
        // outside the path. A ping-pong mover must instead resume along its
        // only valid segment, rather than remain started at an immobile pose.
        let current = usize::from(mover.segment_index);
        let last = mover.waypoints.len().saturating_sub(1);
        mover.direction_sign = match current {
            0 => 1,
            _ if current == last => -1,
            _ => direction,
        };
        return;
    };
    let segment = mover.waypoints[to] - mover.waypoints[from];
    let length = segment.length();
    if !length.is_finite() || length <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH {
        mover.direction_sign = direction;
        return;
    }
    let duration_ms = (length / mover.speed_mps) * 1000.0;
    if !duration_ms.is_finite() || duration_ms <= 0.0 {
        mover.direction_sign = direction;
        return;
    }
    let elapsed = mover.segment_elapsed_ms.clamp(0.0, duration_ms);
    let traversed = elapsed / duration_ms;
    let lower_fraction = if from < to {
        traversed
    } else {
        1.0 - traversed
    };
    let low = from.min(to);
    let high = from.max(to);
    mover.direction_sign = direction;
    if direction > 0 {
        mover.segment_index = low as u16;
        mover.segment_elapsed_ms = lower_fraction * duration_ms;
    } else {
        mover.segment_index = high as u16;
        mover.segment_elapsed_ms = (1.0 - lower_fraction) * duration_ms;
    }
}

fn path_coordinate(mover: &KinematicMoverComponent) -> Option<f32> {
    let (from, to) = segment_indices(mover)?;
    let segment = mover.waypoints[to] - mover.waypoints[from];
    let length = segment.length();
    let duration_ms = (length / mover.speed_mps) * 1000.0;
    if !duration_ms.is_finite() || duration_ms <= 0.0 {
        return None;
    }
    let traversed = (mover.segment_elapsed_ms / duration_ms).clamp(0.0, 1.0);
    Some(if from < to {
        from as f32 + traversed
    } else {
        from as f32 - traversed
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    const EPS: f32 = 1.0e-5;

    fn transform_at(position: Vec3) -> Transform {
        Transform {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    fn sample_mover(mode: KinematicMoverMode, wait_ms: f32) -> KinematicMoverComponent {
        KinematicMoverComponent::new(
            7,
            vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
            vec!["start".to_string(), "finish".to_string()],
            1.0,
            wait_ms,
            mode,
            true,
        )
    }

    fn tick_component(
        component: KinematicMoverComponent,
        transform: Transform,
        dt: f32,
    ) -> (KinematicMoverComponent, Transform, MoverTickStateTable) {
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(transform);
        registry.set_component(id, component).unwrap();
        let mut table = MoverTickStateTable::default();
        run_kinematic_mover_tick(&mut registry, &mut table, dt);
        (
            registry
                .get_component::<KinematicMoverComponent>(id)
                .unwrap()
                .clone(),
            *registry.get_component::<Transform>(id).unwrap(),
            table,
        )
    }

    #[test]
    fn mover_driver_replays_same_seed_deterministically() {
        let seed = sample_mover(KinematicMoverMode::PingPong, 125.0);
        let mut a = (seed.clone(), transform_at(Vec3::ZERO));
        let mut b = (seed, transform_at(Vec3::ZERO));

        for dt in [0.1, 0.25, 0.4, 0.05, 0.33, 0.2] {
            let next_a = tick_component(a.0, a.1, dt);
            let next_b = tick_component(b.0, b.1, dt);
            assert_eq!(next_a.0, next_b.0);
            assert!((next_a.1.position - next_b.1.position).length() < EPS);
            a = (next_a.0, next_a.1);
            b = (next_b.0, next_b.1);
        }
    }

    #[test]
    fn once_stops_at_final_waypoint() {
        let mover = sample_mover(KinematicMoverMode::Once, 0.0);
        let (mover, transform, table) = tick_component(mover, transform_at(Vec3::ZERO), 5.0);

        assert!(mover.completed);
        assert!((transform.position - Vec3::new(2.0, 0.0, 0.0)).length() < EPS);
        let state = table.get(7).expect("mover state should publish");
        assert!((state.tick_delta - Vec3::new(2.0, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn ping_pong_reverses_at_endpoint() {
        let mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        let (mover, transform, _) = tick_component(mover, transform_at(Vec3::ZERO), 2.5);

        assert_eq!(mover.direction_sign, -1);
        assert_eq!(mover.segment_index, 1);
        assert!((transform.position - Vec3::new(1.5, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn endpoint_waits_are_honored() {
        let mover = sample_mover(KinematicMoverMode::PingPong, 500.0);
        let (mover, transform, _) = tick_component(mover, transform_at(Vec3::ZERO), 2.25);

        assert_eq!(mover.direction_sign, -1);
        assert_eq!(mover.segment_index, 1);
        assert!((mover.wait_remaining_ms - 250.0).abs() < EPS);
        assert!((transform.position - Vec3::new(2.0, 0.0, 0.0)).length() < EPS);

        let (mover, transform, _) = tick_component(mover, transform, 0.5);
        assert_eq!(mover.direction_sign, -1);
        assert!((transform.position - Vec3::new(1.75, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn mover_commands_mutate_phase_and_preserve_their_idempotent_edges() {
        let mut mover = sample_mover(KinematicMoverMode::Once, 250.0);
        mover.segment_elapsed_ms = 750.0;
        mover.wait_remaining_ms = 100.0;
        mover.current_linear_velocity = Vec3::X;

        apply_mover_command(&mut mover, &MoverCommand::Stop);
        assert!(!mover.started);
        assert_eq!(mover.segment_elapsed_ms, 750.0);
        assert_eq!(mover.current_linear_velocity, Vec3::ZERO);
        let stopped = mover.clone();
        apply_mover_command(&mut mover, &MoverCommand::Stop);
        assert_eq!(mover, stopped);

        apply_mover_command(&mut mover, &MoverCommand::Start);
        assert!(mover.started);
        assert_eq!(mover.wait_remaining_ms, 0.0);
        assert_eq!(mover.segment_elapsed_ms, 750.0);
        let started = mover.clone();
        apply_mover_command(&mut mover, &MoverCommand::Start);
        assert_eq!(mover, started);

        mover.completed = true;
        let completed = mover.clone();
        apply_mover_command(&mut mover, &MoverCommand::Start);
        assert_eq!(mover, completed);
    }

    #[test]
    fn reverse_reanchors_without_teleporting_and_heads_back() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        let mut transform = transform_at(Vec3::ZERO);
        advance_mover_phase_one_tick(&mut mover, &mut transform, 0.7);
        let position_before_reverse = transform.position;

        apply_mover_command(&mut mover, &MoverCommand::Reverse);
        assert!((position_for_phase(&mover) - position_before_reverse).length() < EPS);
        assert_eq!(mover.direction_sign, -1);

        advance_mover_phase_one_tick(&mut mover, &mut transform, 0.1);
        assert!(
            transform.position.x < position_before_reverse.x,
            "reversed mover should move toward the prior waypoint"
        );
    }

    #[test]
    fn reverse_at_ping_pong_endpoint_resumes_inward() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        let mut transform = transform_at(Vec3::ZERO);

        apply_mover_command(&mut mover, &MoverCommand::Reverse);

        // The normalized reverse phase is segment 1 -> 0 at its destination
        // (the initial endpoint), so the next tick resumes inward.
        assert_eq!(mover.segment_index, 1);
        assert_eq!(mover.direction_sign, -1);
        assert!((position_for_phase(&mover) - Vec3::ZERO).length() < EPS);

        advance_mover_phase_one_tick(&mut mover, &mut transform, 0.1);
        assert!(mover.started);
        assert!(
            transform.position.x > 0.0,
            "a reversed endpoint mover should resume along its valid inward segment"
        );
    }

    #[test]
    fn go_to_initial_path_node_reanchors_mid_segment_without_teleporting() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        let mut transform = transform_at(Vec3::ZERO);
        advance_mover_phase_one_tick(&mut mover, &mut transform, 0.7);
        let position_before_command = transform.position;

        apply_mover_command(&mut mover, &MoverCommand::GoToPathNode("start".to_string()));
        assert_eq!(mover.target_segment, Some(0));
        assert!((position_for_phase(&mover) - position_before_command).length() < EPS);

        advance_mover_phase_one_tick(&mut mover, &mut transform, 0.1);
        assert!(transform.position.x < position_before_command.x);
    }

    #[test]
    fn go_to_path_node_moves_to_named_node_waits_then_holds() {
        let mut mover = KinematicMoverComponent::new(
            7,
            vec![
                Vec3::ZERO,
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.0),
            ],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            1.0,
            250.0,
            KinematicMoverMode::PingPong,
            true,
        );
        let mut transform = transform_at(Vec3::ZERO);

        apply_mover_command(&mut mover, &MoverCommand::GoToPathNode("c".to_string()));
        assert_eq!(mover.target_segment, Some(2));
        advance_mover_phase_one_tick(&mut mover, &mut transform, 4.0);
        assert!((transform.position - Vec3::new(4.0, 0.0, 0.0)).length() < EPS);
        assert_eq!(mover.target_segment, Some(2));
        assert!(!mover.completed);

        advance_mover_phase_one_tick(&mut mover, &mut transform, 0.25);
        assert_eq!(mover.target_segment, None);
        assert!(mover.completed);
        assert!((transform.position - Vec3::new(4.0, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn unknown_path_node_preserves_existing_target_and_phase() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        mover.target_segment = Some(1);
        mover.segment_elapsed_ms = 500.0;
        let before = mover.clone();

        apply_mover_command(
            &mut mover,
            &MoverCommand::GoToPathNode("missing".to_string()),
        );

        assert_eq!(mover, before);
    }

    #[test]
    fn mover_driver_replays_targeted_motion_deterministically() {
        let mut seed = KinematicMoverComponent::new(
            7,
            vec![
                Vec3::ZERO,
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.0),
            ],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            1.0,
            100.0,
            KinematicMoverMode::PingPong,
            true,
        );
        apply_mover_command(&mut seed, &MoverCommand::GoToPathNode("c".to_string()));
        let mut a = (seed.clone(), transform_at(Vec3::ZERO));
        let mut b = (seed, transform_at(Vec3::ZERO));

        for dt in [0.1, 0.25, 0.4, 0.05, 0.33, 0.2, 3.0] {
            let next_a = tick_component(a.0, a.1, dt);
            let next_b = tick_component(b.0, b.1, dt);
            assert_eq!(next_a.0, next_b.0);
            assert!((next_a.1.position - next_b.1.position).length() < EPS);
            a = (next_a.0, next_a.1);
            b = (next_b.0, next_b.1);
        }
    }

    #[test]
    fn command_target_applier_skips_non_movers() {
        let mut registry = EntityRegistry::new();
        let mover_entity = registry.spawn(transform_at(Vec3::ZERO));
        registry
            .set_component(
                mover_entity,
                sample_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();
        let non_mover = registry.spawn(transform_at(Vec3::ONE));
        registry
            .set_tags(mover_entity, vec!["lift_group".to_string()])
            .unwrap();
        registry
            .set_tags(non_mover, vec!["lift_group".to_string()])
            .unwrap();

        apply_mover_command_to_targets(
            &mut registry,
            &[mover_entity, non_mover],
            &MoverCommand::Stop,
        );

        assert!(
            registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .is_ok()
        );
        assert!(
            !registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap()
                .started
        );
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(non_mover)
                .is_err()
        );
    }

    #[test]
    fn script_mover_primitive_matches_shared_kvp_command_path() {
        let mut script_registry = EntityRegistry::new();
        let script_target = script_registry.spawn(transform_at(Vec3::ZERO));
        script_registry
            .set_component(
                script_target,
                sample_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();

        let mut kvp_registry = EntityRegistry::new();
        let kvp_target = kvp_registry.spawn(transform_at(Vec3::ZERO));
        assert_eq!(script_target, kvp_target, "fixture registries must align");
        kvp_registry
            .set_component(kvp_target, sample_mover(KinematicMoverMode::PingPong, 0.0))
            .unwrap();
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_mover_reaction_primitives(&mut reactions);
        assert!(
            reactions
                .dispatch(
                    "moverGoToPathNode",
                    &mut script_registry,
                    &[script_target],
                    &serde_json::json!({ "node": "finish" }),
                )
                .unwrap()
        );
        apply_mover_command_to_targets(
            &mut kvp_registry,
            &[kvp_target],
            &MoverCommand::GoToPathNode("finish".to_string()),
        );

        assert_eq!(
            script_registry
                .get_component::<KinematicMoverComponent>(script_target)
                .unwrap(),
            kvp_registry
                .get_component::<KinematicMoverComponent>(kvp_target)
                .unwrap(),
            "the script primitive must use the same mover-phase applier as KVP commands"
        );
    }

    #[test]
    fn near_zero_ping_pong_segment_completes_without_spinning() {
        let mover = KinematicMoverComponent::new(
            7,
            vec![
                Vec3::ZERO,
                Vec3::new(KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH * 0.5, 0.0, 0.0),
            ],
            vec!["start".to_string(), "finish".to_string()],
            1.0,
            0.0,
            KinematicMoverMode::PingPong,
            true,
        );

        let (mover, transform, table) = tick_component(mover, transform_at(Vec3::ZERO), 1.0);

        assert!(mover.completed);
        assert_eq!(mover.segment_index, 1);
        assert!(
            transform.position.length() <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH,
            "degenerate mover should stop at the bad segment endpoint, got {:?}",
            transform.position
        );
        let state = table.get(7).expect("mover state should publish");
        assert!(
            state.tick_delta.length() <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH,
            "degenerate mover should not accumulate tick delta, got {:?}",
            state.tick_delta
        );
    }
}
