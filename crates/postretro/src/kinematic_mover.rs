//! Fixed-tick deterministic kinematic mover driver.
//!
//! The driver is intentionally a pure function of component phase, static path
//! data, and fixed `dt`. It reads no clock, RNG, host role, or external state.

use std::collections::HashMap;

use crate::collision::moving::{MoverPose, MoverPoseSource};
use glam::{Quat, Vec3};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverMode, Transform,
};
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;

mod auto_close;
mod blocking;
mod commands;

pub(crate) use auto_close::MoverAutoCloseTimers;
pub(crate) use blocking::{MoverBlockingState, MoverEventKind, run_mover_blocking_pass};
#[cfg(test)]
pub(crate) use commands::apply_mover_command;
pub(crate) use commands::{
    MoverCommandDiagnostics, MoverSetSpinRateArgs, apply_mover_command_to_known_movers,
    apply_mover_command_to_targets, register_mover_reaction_primitives,
    register_sequenced_mover_primitives,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoverTickState {
    pub(crate) entity: EntityId,
    pub(crate) transform: Transform,
    pub(crate) linear_velocity: Vec3,
    pub(crate) tick_delta: Vec3,
    pub(crate) angular_velocity: Vec3,
    pub(crate) tick_rotation_delta: Quat,
    pub(crate) carry_yaw: bool,
    pub(crate) tick_dt: f32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MoverEndpointArrivals {
    opened: bool,
    closed: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MoverTickStateTable {
    states: HashMap<u32, MoverTickState>,
    endpoint_arrivals: Vec<(u32, MoverEndpointArrivals)>,
    blocking_state: MoverBlockingState,
    mover_entities: Vec<EntityId>,
}

impl MoverTickStateTable {
    pub(crate) fn clear(&mut self) {
        self.states.clear();
        self.endpoint_arrivals.clear();
        self.blocking_state.clear();
        self.mover_entities.clear();
    }

    fn begin_tick(&mut self) {
        self.states.clear();
        self.endpoint_arrivals.clear();
        self.mover_entities.clear();
    }

    pub(crate) fn publish(&mut self, mover_id: u32, state: MoverTickState) {
        self.states.insert(mover_id, state);
    }

    pub(crate) fn get(&self, mover_id: u32) -> Option<&MoverTickState> {
        self.states.get(&mover_id)
    }

    pub(crate) fn terminus_events(&self) -> impl Iterator<Item = (MoverEventKind, u32)> + '_ {
        self.endpoint_arrivals
            .iter()
            .flat_map(|(mover_id, arrivals)| {
                [
                    arrivals
                        .opened
                        .then_some((MoverEventKind::Opened, *mover_id)),
                    arrivals
                        .closed
                        .then_some((MoverEventKind::Closed, *mover_id)),
                ]
                .into_iter()
                .flatten()
            })
    }

    /// Split the tick-local pose view from the host-only policy timers. The
    /// collision pass needs both after motion is published. Per-tick reset is
    /// separate from `clear()`, which is the full level-lifetime reset.
    pub(crate) fn split_for_blocking(
        &mut self,
    ) -> (MoverTickPoseSource<'_>, &mut MoverBlockingState) {
        (
            MoverTickPoseSource {
                states: &self.states,
                endpoint_arrivals: &self.endpoint_arrivals,
            },
            &mut self.blocking_state,
        )
    }
}

pub(crate) struct MoverTickPoseSource<'a> {
    states: &'a HashMap<u32, MoverTickState>,
    endpoint_arrivals: &'a [(u32, MoverEndpointArrivals)],
}

impl MoverPoseSource for MoverTickPoseSource<'_> {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.states.get(&mover_id).map(|state| MoverPose {
            transform: state.transform,
            linear_velocity: state.linear_velocity,
            tick_delta: state.tick_delta,
            angular_velocity: state.angular_velocity,
            tick_rotation_delta: state.tick_rotation_delta,
            carry_yaw: state.carry_yaw,
            tick_dt: state.tick_dt,
        })
    }

    fn had_endpoint_arrival(&self, mover_id: u32) -> bool {
        self.endpoint_arrivals.iter().any(|(id, _)| *id == mover_id)
    }
}

impl MoverPoseSource for MoverTickStateTable {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.get(mover_id).map(|state| MoverPose {
            transform: state.transform,
            linear_velocity: state.linear_velocity,
            tick_delta: state.tick_delta,
            angular_velocity: state.angular_velocity,
            tick_rotation_delta: state.tick_rotation_delta,
            carry_yaw: state.carry_yaw,
            tick_dt: state.tick_dt,
        })
    }

    fn had_endpoint_arrival(&self, mover_id: u32) -> bool {
        self.endpoint_arrivals.iter().any(|(id, _)| *id == mover_id)
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
    side_table.begin_tick();
    side_table.mover_entities.extend(
        registry
            .iter_with_kind(ComponentKind::KinematicMover)
            .map(|(id, _)| id),
    );

    for index in 0..side_table.mover_entities.len() {
        let entity = side_table.mover_entities[index];
        let Ok(mut transform) = registry.get_component::<Transform>(entity).copied() else {
            continue;
        };
        let (mover_id, carry_yaw, pose, endpoint_arrivals) = {
            let Ok(ComponentValue::KinematicMover(mover)) =
                registry.get_component_value_mut(entity, ComponentKind::KinematicMover)
            else {
                continue;
            };
            let (pose, endpoint_arrivals) =
                advance_mover_phase_one_tick_with_arrivals(mover, &mut transform, tick_dt);
            (mover.mover_id, mover.carry_yaw, pose, endpoint_arrivals)
        };

        let _ = registry.set_component(entity, transform);
        side_table.publish(
            mover_id,
            MoverTickState {
                entity,
                transform,
                linear_velocity: pose.linear_velocity,
                tick_delta: pose.tick_delta,
                angular_velocity: pose.angular_velocity,
                tick_rotation_delta: pose.tick_rotation_delta,
                carry_yaw,
                tick_dt: pose.tick_dt,
            },
        );
        if endpoint_arrivals != MoverEndpointArrivals::default() {
            side_table
                .endpoint_arrivals
                .push((mover_id, endpoint_arrivals));
        }
    }
}

pub(crate) fn mover_pose_for_current_phase(
    transform: Transform,
    mover: &KinematicMoverComponent,
    tick_dt: f32,
) -> MoverPose {
    let mut transform = transform;
    transform.position = position_for_phase(mover);
    if mover.spin_angle_rad != 0.0
        || mover.spin_rate_rad_s != 0.0
        || mover.spin_target_rate_rad_s != 0.0
    {
        transform.rotation = Quat::from_axis_angle(mover.spin_axis, mover.spin_angle_rad);
    }
    let linear_velocity = mover.current_linear_velocity;
    let (angular_velocity, tick_rotation_delta) = angular_kinematics_for_current_phase(mover);
    MoverPose {
        transform,
        linear_velocity,
        tick_delta: if tick_dt.is_finite() && tick_dt > 0.0 {
            linear_velocity * tick_dt
        } else {
            Vec3::ZERO
        },
        angular_velocity,
        tick_rotation_delta,
        carry_yaw: mover.carry_yaw,
        tick_dt,
    }
}

pub(crate) fn advance_mover_phase_one_tick(
    mover: &mut KinematicMoverComponent,
    transform: &mut Transform,
    tick_dt: f32,
) -> MoverPose {
    advance_mover_phase_one_tick_with_arrivals(mover, transform, tick_dt).0
}

fn advance_mover_phase_one_tick_with_arrivals(
    mover: &mut KinematicMoverComponent,
    transform: &mut Transform,
    tick_dt: f32,
) -> (MoverPose, MoverEndpointArrivals) {
    let mut endpoint_arrivals = MoverEndpointArrivals::default();
    // Completion and restart own stale-hold cleanup across every peer. A client
    // only reconciles the host's phase, so it must never retain a completed hold.
    if mover.completed {
        mover.blocked = false;
    }
    if mover.blocked {
        return (
            blocked_mover_pose(mover, transform, tick_dt),
            endpoint_arrivals,
        );
    }
    let (angular_velocity, tick_rotation_delta) = advance_spin_phase(mover, transform, tick_dt);
    let start_position = position_for_phase(mover);
    transform.position = start_position;
    let end_position = advance_mover(mover, tick_dt, &mut endpoint_arrivals);
    transform.position = end_position;
    if mover.completed {
        mover.blocked = false;
    }
    let tick_delta = end_position - start_position;
    let linear_velocity = if tick_dt.is_finite() && tick_dt > 0.0 {
        tick_delta / tick_dt
    } else {
        Vec3::ZERO
    };
    mover.current_linear_velocity = linear_velocity;
    let pose = MoverPose {
        transform: *transform,
        linear_velocity,
        tick_delta,
        angular_velocity,
        tick_rotation_delta,
        carry_yaw: mover.carry_yaw,
        tick_dt,
    };
    (pose, endpoint_arrivals)
}

/// Publish a zero-motion tick while a host-authoritative stop hold is active.
/// The driver still refreshes provenance so carry and replay cannot reuse the
/// prior tick's rotation or velocity.
fn blocked_mover_pose(
    mover: &mut KinematicMoverComponent,
    transform: &mut Transform,
    tick_dt: f32,
) -> MoverPose {
    mover.spin_angle_before_tick_rad = mover.spin_angle_rad;
    mover.was_active_this_tick = false;
    mover.current_linear_velocity = Vec3::ZERO;
    transform.position = position_for_phase(mover);
    if mover.spin_axis != Vec3::ZERO {
        transform.rotation = Quat::from_axis_angle(mover.spin_axis, mover.spin_angle_rad);
    }
    MoverPose {
        transform: *transform,
        linear_velocity: Vec3::ZERO,
        tick_delta: Vec3::ZERO,
        angular_velocity: Vec3::ZERO,
        tick_rotation_delta: Quat::IDENTITY,
        carry_yaw: mover.carry_yaw,
        tick_dt,
    }
}

fn advance_spin_phase(
    mover: &mut KinematicMoverComponent,
    transform: &mut Transform,
    tick_dt: f32,
) -> (Vec3, Quat) {
    mover.spin_angle_before_tick_rad = mover.spin_angle_rad;
    let has_spin_work = mover.spin_rate_rad_s != 0.0 || mover.spin_target_rate_rad_s != 0.0;
    let has_valid_tick = tick_dt.is_finite() && tick_dt > 0.0;
    mover.was_active_this_tick = mover.started && !mover.completed && has_valid_tick;
    if !mover.was_active_this_tick || !has_spin_work {
        return (Vec3::ZERO, Quat::IDENTITY);
    }

    let start_rotation = transform.rotation;
    mover.spin_rate_rad_s = ramp_spin_rate(
        mover.spin_rate_rad_s,
        mover.spin_target_rate_rad_s,
        mover.spin_accel_rad_s2,
        tick_dt,
    );
    let tick_angle_rad = mover.spin_rate_rad_s * tick_dt;
    mover.spin_angle_rad =
        (mover.spin_angle_rad + tick_angle_rad).rem_euclid(std::f32::consts::TAU);
    let end_rotation = Quat::from_axis_angle(mover.spin_axis, mover.spin_angle_rad);
    transform.rotation = end_rotation;
    (
        mover.spin_axis * mover.spin_rate_rad_s,
        end_rotation * start_rotation.inverse(),
    )
}

fn ramp_spin_rate(current_rate: f32, target_rate: f32, accel_rad_s2: f32, tick_dt: f32) -> f32 {
    if accel_rad_s2 == 0.0 {
        return target_rate;
    }

    let max_change = accel_rad_s2 * tick_dt;
    if current_rate < target_rate {
        (current_rate + max_change).min(target_rate)
    } else {
        (current_rate - max_change).max(target_rate)
    }
}

fn angular_kinematics_for_current_phase(mover: &KinematicMoverComponent) -> (Vec3, Quat) {
    if !mover.was_active_this_tick || mover.spin_rate_rad_s == 0.0 {
        return (Vec3::ZERO, Quat::IDENTITY);
    }

    let angular_velocity = mover.spin_axis * mover.spin_rate_rad_s;
    let start_rotation = Quat::from_axis_angle(mover.spin_axis, mover.spin_angle_before_tick_rad);
    let end_rotation = Quat::from_axis_angle(mover.spin_axis, mover.spin_angle_rad);
    let tick_rotation_delta = end_rotation * start_rotation.inverse();
    (angular_velocity, tick_rotation_delta)
}

fn advance_mover(
    mover: &mut KinematicMoverComponent,
    tick_dt: f32,
    endpoint_arrivals: &mut MoverEndpointArrivals,
) -> Vec3 {
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
            let last = mover.waypoints.len().saturating_sub(1);
            endpoint_arrivals.opened |= mover.direction_sign > 0 && to_index == last;
            endpoint_arrivals.closed |= mover.direction_sign < 0 && to_index == 0;
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

/// Host-only directional intent used when an automatic-return timer expires.
///
/// Automatic closing is not a blind reversal: every path, including a completed
/// once mover and a held ping-pong mover, resolves to the closed endpoint at
/// index zero. The resulting phase is replicated; the timer that chose it is
/// deliberately not.
fn travel_toward_closed_terminus(mover: &mut KinematicMoverComponent) {
    if mover.waypoints.len() < 2 {
        return;
    }
    reanchor_direction(mover, -1);
    mover.target_segment = Some(0);
    mover.started = true;
    mover.completed = false;
    mover.blocked = false;
    mover.wait_remaining_ms = 0.0;
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
    use postretro_entities::MoverCommand;

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
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 1.0,
                wait_ms,
                mode,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
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

    fn assert_vec3_approx(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < EPS,
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn assert_quat_approx(actual: Quat, expected: Quat) {
        assert!(
            (actual.dot(expected).abs() - 1.0).abs() < EPS,
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn pure_rotator(
        initial_spin_rate_rad_s: f32,
        spin_accel_rad_s2: f32,
    ) -> KinematicMoverComponent {
        KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO],
                waypoint_names: vec!["origin".to_string()],
                speed_mps: 0.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: true,
                spin_axis: Vec3::Y,
                initial_spin_rate_rad_s,
                spin_accel_rad_s2,
                carry_yaw: true,
            },
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
    fn blocked_mover_holds_phase_and_publishes_zero_motion_provenance() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        mover.segment_elapsed_ms = 500.0;
        mover.current_linear_velocity = Vec3::X;
        mover.spin_axis = Vec3::Y;
        mover.spin_angle_rad = 0.75;
        mover.spin_rate_rad_s = 2.0;
        mover.spin_target_rate_rad_s = 2.0;
        mover.was_active_this_tick = true;
        mover.blocked = true;
        let mut transform = transform_at(Vec3::ZERO);

        let pose = advance_mover_phase_one_tick(&mut mover, &mut transform, 0.25);

        assert_vec3_approx(transform.position, Vec3::new(0.5, 0.0, 0.0));
        assert_quat_approx(transform.rotation, Quat::from_rotation_y(0.75));
        assert!(mover.blocked);
        assert!((mover.segment_elapsed_ms - 500.0).abs() < EPS);
        assert_eq!(mover.current_linear_velocity, Vec3::ZERO);
        assert_eq!(mover.spin_angle_before_tick_rad, 0.75);
        assert!(!mover.was_active_this_tick);
        assert_eq!(pose.tick_delta, Vec3::ZERO);
        assert_eq!(pose.linear_velocity, Vec3::ZERO);
        assert_eq!(pose.angular_velocity, Vec3::ZERO);
        assert_eq!(pose.tick_rotation_delta, Quat::IDENTITY);
    }

    #[test]
    fn completed_mover_clears_stale_block_before_its_noop_tick() {
        let mut mover = sample_mover(KinematicMoverMode::Once, 0.0);
        mover.completed = true;
        mover.blocked = true;
        let mut transform = transform_at(Vec3::ZERO);

        let pose = advance_mover_phase_one_tick(&mut mover, &mut transform, 0.25);

        assert!(!mover.blocked);
        assert_eq!(pose.tick_delta, Vec3::ZERO);
        assert_eq!(pose.linear_velocity, Vec3::ZERO);
    }

    #[test]
    fn spin_driver_is_deterministic_and_wraps_angle() {
        let mut a = (
            pure_rotator(std::f32::consts::TAU + 0.5, 0.0),
            transform_at(Vec3::ZERO),
        );
        let mut b = a.clone();

        for dt in [0.25, 1.0, 0.5, 0.75] {
            let next_a = tick_component(a.0, a.1, dt);
            let next_b = tick_component(b.0, b.1, dt);
            assert!((next_a.0.spin_angle_rad - next_b.0.spin_angle_rad).abs() < EPS);
            assert_quat_approx(next_a.1.rotation, next_b.1.rotation);
            a = (next_a.0, next_a.1);
            b = (next_b.0, next_b.1);
        }

        let (mover, transform, _) = tick_component(
            pure_rotator(std::f32::consts::TAU + 0.5, 0.0),
            transform_at(Vec3::ZERO),
            1.0,
        );
        assert!((mover.spin_angle_rad - 0.5).abs() < EPS);
        assert!((0.0..std::f32::consts::TAU).contains(&mover.spin_angle_rad));
        assert_quat_approx(transform.rotation, Quat::from_rotation_y(0.5));
    }

    #[test]
    fn mid_spin_phase_replay_reproduces_orientation_and_pose_kinematics() {
        let (mid_phase, mid_transform, _) =
            tick_component(pure_rotator(2.0, 0.0), transform_at(Vec3::ZERO), 0.35);
        let replay_pose = mover_pose_for_current_phase(mid_transform, &mid_phase, 0.2);
        assert_vec3_approx(replay_pose.angular_velocity, Vec3::Y * 2.0);
        assert_quat_approx(replay_pose.tick_rotation_delta, Quat::from_rotation_y(0.7));
        assert!(replay_pose.carry_yaw);

        let mut a = (mid_phase.clone(), mid_transform);
        let mut b = (mid_phase, mid_transform);
        for dt in [0.1, 0.3, 0.2] {
            let next_a = tick_component(a.0, a.1, dt);
            let next_b = tick_component(b.0, b.1, dt);
            assert!((next_a.0.spin_angle_rad - next_b.0.spin_angle_rad).abs() < EPS);
            assert_vec3_approx(next_a.2.get(7).unwrap().angular_velocity, Vec3::Y * 2.0);
            assert_quat_approx(next_a.1.rotation, next_b.1.rotation);
            a = (next_a.0, next_a.1);
            b = (next_b.0, next_b.1);
        }
    }

    #[test]
    fn spin_rate_ramp_clamps_without_overshoot_and_snaps_at_zero_acceleration() {
        let mut ramped = pure_rotator(0.0, 4.0);
        ramped.spin_target_rate_rad_s = 3.0;
        let (ramped, transform, table) = tick_component(ramped, transform_at(Vec3::ZERO), 0.5);
        assert!((ramped.spin_rate_rad_s - 2.0).abs() < EPS);
        assert!((ramped.spin_angle_rad - 1.0).abs() < EPS);
        assert_vec3_approx(table.get(7).unwrap().angular_velocity, Vec3::Y * 2.0);
        assert_quat_approx(transform.rotation, Quat::from_rotation_y(1.0));

        let (ramped, transform, _) = tick_component(ramped, transform, 0.5);
        assert!((ramped.spin_rate_rad_s - 3.0).abs() < EPS);
        assert!((ramped.spin_angle_rad - 2.5).abs() < EPS);
        assert_quat_approx(transform.rotation, Quat::from_rotation_y(2.5));

        let mut snapped = pure_rotator(1.0, 0.0);
        snapped.spin_target_rate_rad_s = 4.0;
        let (snapped, transform, _) = tick_component(snapped, transform_at(Vec3::ZERO), 0.25);
        assert!((snapped.spin_rate_rad_s - 4.0).abs() < EPS);
        assert!((snapped.spin_angle_rad - 1.0).abs() < EPS);
        assert_quat_approx(transform.rotation, Quat::from_rotation_y(1.0));

        let mut reversing = pure_rotator(2.0, 3.0);
        reversing.spin_target_rate_rad_s = -1.0;
        let (reversing, transform, _) = tick_component(reversing, transform_at(Vec3::ZERO), 0.5);
        assert!((reversing.spin_rate_rad_s - 0.5).abs() < EPS);
        assert!((reversing.spin_angle_rad - 0.25).abs() < EPS);

        let (reversing, _, _) = tick_component(reversing, transform, 0.5);
        assert!((reversing.spin_rate_rad_s + 1.0).abs() < EPS);
    }

    #[test]
    fn set_spin_rate_ramps_through_zero_to_signed_target_and_snaps_without_acceleration() {
        let mut reversing = pure_rotator(std::f32::consts::PI, std::f32::consts::PI);
        apply_mover_command(&mut reversing, &MoverCommand::SetSpinRate(-180.0));
        assert!((reversing.spin_target_rate_rad_s + std::f32::consts::PI).abs() < EPS);

        let (reversing, transform, _) = tick_component(reversing, transform_at(Vec3::ZERO), 0.5);
        assert!((reversing.spin_rate_rad_s - std::f32::consts::FRAC_PI_2).abs() < EPS);

        let (reversing, transform, _) = tick_component(reversing, transform, 0.5);
        assert!(reversing.spin_rate_rad_s.abs() < EPS);

        let (reversing, _, _) = tick_component(reversing, transform, 0.5);
        assert!((reversing.spin_rate_rad_s + std::f32::consts::FRAC_PI_2).abs() < EPS);

        let mut snapped = pure_rotator(1.0, 0.0);
        apply_mover_command(&mut snapped, &MoverCommand::SetSpinRate(90.0));
        assert!((snapped.spin_rate_rad_s - 1.0).abs() < EPS);
        let (snapped, _, _) = tick_component(snapped, transform_at(Vec3::ZERO), 0.25);
        assert!((snapped.spin_rate_rad_s - std::f32::consts::FRAC_PI_2).abs() < EPS);
    }

    #[test]
    fn mid_ramp_phase_replay_reproduces_rate_and_orientation() {
        let mut seed = pure_rotator(0.0, 1.5);
        seed.spin_target_rate_rad_s = 4.0;
        let (mid_phase, mid_transform, _) = tick_component(seed, transform_at(Vec3::ZERO), 0.75);
        assert!((mid_phase.spin_rate_rad_s - 1.125).abs() < EPS);

        let mut a = (mid_phase.clone(), mid_transform);
        let mut b = (mid_phase, mid_transform);
        for dt in [0.25, 0.5, 0.75] {
            let next_a = tick_component(a.0, a.1, dt);
            let next_b = tick_component(b.0, b.1, dt);
            assert!((next_a.0.spin_rate_rad_s - next_b.0.spin_rate_rad_s).abs() < EPS);
            assert!(
                (next_a.0.spin_target_rate_rad_s - next_b.0.spin_target_rate_rad_s).abs() < EPS
            );
            assert!((next_a.0.spin_angle_rad - next_b.0.spin_angle_rad).abs() < EPS);
            assert_quat_approx(next_a.1.rotation, next_b.1.rotation);
            a = (next_a.0, next_a.1);
            b = (next_b.0, next_b.1);
        }
    }

    #[test]
    fn spin_and_translation_compose_into_one_mover_pose() {
        let mover = KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::Y,
                initial_spin_rate_rad_s: std::f32::consts::PI,
                spin_accel_rad_s2: 0.0,
                carry_yaw: true,
            },
        );

        let (mover, transform, table) = tick_component(mover, transform_at(Vec3::ZERO), 0.5);
        let state = table.get(7).unwrap();
        assert_vec3_approx(transform.position, Vec3::new(0.5, 0.0, 0.0));
        assert_quat_approx(
            transform.rotation,
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        assert_vec3_approx(state.tick_delta, Vec3::new(0.5, 0.0, 0.0));
        assert_vec3_approx(state.angular_velocity, Vec3::Y * std::f32::consts::PI);
        assert_quat_approx(
            state.tick_rotation_delta,
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        assert!(state.carry_yaw);
        assert!(!mover.completed);
    }

    #[test]
    fn pure_rotator_spins_without_completing_its_non_traversable_path() {
        let (mover, transform, table) =
            tick_component(pure_rotator(2.0, 0.0), transform_at(Vec3::ZERO), 1.0);
        assert!(!mover.completed);
        assert_vec3_approx(transform.position, Vec3::ZERO);
        assert_quat_approx(transform.rotation, Quat::from_rotation_y(2.0));
        assert_vec3_approx(table.get(7).unwrap().angular_velocity, Vec3::Y * 2.0);
    }

    // Regression: linear completion used to erase the rotation already applied
    // earlier in the same tick from the published mover pose.
    #[test]
    fn once_movers_publish_final_tick_rotation_then_stop() {
        let once = KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: true,
                spin_axis: Vec3::Y,
                initial_spin_rate_rad_s: 1.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        let (once, transform, table) = tick_component(once, transform_at(Vec3::ZERO), 1.0);
        assert!(once.completed);
        assert!((once.spin_angle_rad - 1.0).abs() < EPS);
        let live_completion_pose = table.pose(7).unwrap();
        assert_vec3_approx(live_completion_pose.angular_velocity, Vec3::Y);
        assert_quat_approx(
            live_completion_pose.tick_rotation_delta,
            Quat::from_rotation_y(1.0),
        );
        let reconstructed_completion_pose = mover_pose_for_current_phase(transform, &once, 1.0);
        assert_eq!(
            live_completion_pose.transform,
            reconstructed_completion_pose.transform
        );
        assert_vec3_approx(
            live_completion_pose.tick_delta,
            reconstructed_completion_pose.tick_delta,
        );
        assert_vec3_approx(
            reconstructed_completion_pose.angular_velocity,
            live_completion_pose.angular_velocity,
        );
        assert_quat_approx(
            reconstructed_completion_pose.tick_rotation_delta,
            live_completion_pose.tick_rotation_delta,
        );

        let (once, transform_after_stop, table) = tick_component(once, transform, 0.5);
        assert!((once.spin_angle_rad - 1.0).abs() < EPS);
        assert_quat_approx(transform_after_stop.rotation, transform.rotation);
        assert_vec3_approx(table.get(7).unwrap().angular_velocity, Vec3::ZERO);
        assert_quat_approx(table.get(7).unwrap().tick_rotation_delta, Quat::IDENTITY);
        let completed_pose = mover_pose_for_current_phase(transform_after_stop, &once, 0.5);
        assert_vec3_approx(completed_pose.angular_velocity, Vec3::ZERO);
        assert_quat_approx(completed_pose.tick_rotation_delta, Quat::IDENTITY);

        let ping_pong = KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::Y,
                initial_spin_rate_rad_s: 1.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        let (ping_pong, transform, _) = tick_component(ping_pong, transform_at(Vec3::ZERO), 1.0);
        let (ping_pong, _, table) = tick_component(ping_pong, transform, 0.5);
        assert!(!ping_pong.completed);
        assert!((ping_pong.spin_angle_rad - 1.5).abs() < EPS);
        assert_vec3_approx(table.get(7).unwrap().angular_velocity, Vec3::Y);
    }

    // Regression: replay gated the just-finished tick through post-command
    // started/completed flags, so transition snapshots described a different tick
    // from the live carry table.
    #[test]
    fn post_command_phase_reconstructs_the_tick_that_actually_ran() {
        for command in [
            MoverCommand::Stop,
            MoverCommand::Reverse,
            MoverCommand::GoToPathNode("start".to_string()),
        ] {
            let mover = KinematicMoverComponent::new(
                7,
                postretro_entities::KinematicMoverConfig {
                    waypoints: vec![Vec3::ZERO, Vec3::X],
                    waypoint_names: vec!["start".to_string(), "finish".to_string()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: KinematicMoverMode::PingPong,
                    started: true,
                    spin_axis: Vec3::Y,
                    initial_spin_rate_rad_s: 1.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: true,
                },
            );
            let (mut mover, transform, table) =
                tick_component(mover, transform_at(Vec3::ZERO), 0.25);
            let live = table.pose(7).unwrap();

            apply_mover_command(&mut mover, &command);
            let replay = mover_pose_for_current_phase(transform, &mover, 0.25);

            assert_vec3_approx(replay.tick_delta, live.tick_delta);
            assert_vec3_approx(replay.angular_velocity, live.angular_velocity);
            assert_quat_approx(replay.tick_rotation_delta, live.tick_rotation_delta);
        }

        let mut stopped = pure_rotator(1.0, 0.0);
        stopped.started = false;
        let (mut stopped, transform, table) =
            tick_component(stopped, transform_at(Vec3::ZERO), 0.25);
        assert_quat_approx(table.pose(7).unwrap().tick_rotation_delta, Quat::IDENTITY);
        apply_mover_command(&mut stopped, &MoverCommand::Start);
        let replay = mover_pose_for_current_phase(transform, &stopped, 0.25);
        assert_quat_approx(replay.tick_rotation_delta, Quat::IDENTITY);
        assert_vec3_approx(replay.angular_velocity, Vec3::ZERO);
    }

    // Regression: rate*dt reported rotation that the wrapped f32 phase did not
    // actually apply, especially below phase precision and at huge finite rates.
    #[test]
    fn tick_rotation_delta_matches_the_transform_rotation_exactly() {
        for (start_angle, rate, dt) in [(1.0, f32::EPSILON * 0.25, 0.25), (5.75, 3.0e30, 1.0e-20)] {
            let mut mover = pure_rotator(rate, 0.0);
            mover.spin_angle_rad = start_angle;
            let start_rotation = Quat::from_rotation_y(start_angle);
            let start_transform = Transform {
                rotation: start_rotation,
                ..transform_at(Vec3::ZERO)
            };
            let (_, transform, table) = tick_component(mover, start_transform, dt);
            let delta = table.get(7).unwrap().tick_rotation_delta;

            assert_quat_approx(delta * start_rotation, transform.rotation);
        }
    }

    #[test]
    fn stop_freezes_spin_and_start_resumes_retained_rate_phase() {
        let mut mover = pure_rotator(std::f32::consts::PI, std::f32::consts::PI);
        apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(0.0));
        assert!(
            mover.spin_rate_rad_s > 0.0,
            "set_spin_rate(0) must not hard-stop"
        );
        assert!(mover.spin_target_rate_rad_s.abs() < EPS);
        let (mut mover, transform, _) = tick_component(mover, transform_at(Vec3::ZERO), 0.25);
        assert!((mover.spin_rate_rad_s - std::f32::consts::FRAC_PI_2 * 1.5).abs() < EPS);

        apply_mover_command(&mut mover, &MoverCommand::Stop);
        let retained_rate = mover.spin_rate_rad_s;
        let retained_target = mover.spin_target_rate_rad_s;
        let retained_angle = mover.spin_angle_rad;
        let (mut mover, frozen_transform, table) = tick_component(mover, transform, 0.5);
        assert!((mover.spin_rate_rad_s - retained_rate).abs() < EPS);
        assert!((mover.spin_target_rate_rad_s - retained_target).abs() < EPS);
        assert!((mover.spin_angle_rad - retained_angle).abs() < EPS);
        assert_quat_approx(frozen_transform.rotation, transform.rotation);
        assert_vec3_approx(table.get(7).unwrap().angular_velocity, Vec3::ZERO);
        assert_quat_approx(table.get(7).unwrap().tick_rotation_delta, Quat::IDENTITY);
        let stopped_replay_pose = mover_pose_for_current_phase(frozen_transform, &mover, 0.5);
        assert_vec3_approx(stopped_replay_pose.angular_velocity, Vec3::ZERO);
        assert_quat_approx(stopped_replay_pose.tick_rotation_delta, Quat::IDENTITY);

        apply_mover_command(&mut mover, &MoverCommand::Start);
        let (mover, resumed_transform, table) = tick_component(mover, frozen_transform, 0.5);
        assert!(
            mover.spin_rate_rad_s > 0.0,
            "resumed mover should keep ramping to rest"
        );
        assert!(mover.spin_rate_rad_s < retained_rate);
        assert!(mover.spin_target_rate_rad_s.abs() < EPS);
        assert!(mover.spin_angle_rad > retained_angle);
        assert_quat_approx(
            resumed_transform.rotation,
            Quat::from_rotation_y(mover.spin_angle_rad),
        );
        assert_vec3_approx(
            table.get(7).unwrap().angular_velocity,
            Vec3::Y * mover.spin_rate_rad_s,
        );
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

    // Regression: a fast ping-pong mover could cross both termini and finish
    // between them, so post-phase comparison lost the Opened edge entirely.
    #[test]
    fn high_speed_ping_pong_publishes_each_endpoint_arrival_once() {
        let mover = sample_mover(KinematicMoverMode::PingPong, 0.0);

        let (_, _, table) = tick_component(mover, transform_at(Vec3::ZERO), 4.5);
        let events: Vec<_> = table.terminus_events().collect();

        assert_eq!(
            events
                .iter()
                .filter(|(kind, _)| *kind == MoverEventKind::Opened)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|(kind, _)| *kind == MoverEventKind::Closed)
                .count(),
            1
        );
    }

    // Regression: level unload cleared tick poses but retained crusher cadence
    // under a reused mover/victim identity pair.
    #[test]
    fn level_clear_drops_blocking_cadence_as_well_as_tick_poses() {
        let mut table = MoverTickStateTable::default();
        let mover = EntityId::new(1, 0);
        let victim = EntityId::new(2, 0);
        table.blocking_state.seed_test_cadence(mover, victim);

        table.clear();

        assert!(table.blocking_state.is_empty());
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
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![
                    Vec3::ZERO,
                    Vec3::new(2.0, 0.0, 0.0),
                    Vec3::new(4.0, 0.0, 0.0),
                ],
                waypoint_names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                speed_mps: 1.0,
                wait_ms: 250.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
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
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![
                    Vec3::ZERO,
                    Vec3::new(2.0, 0.0, 0.0),
                    Vec3::new(4.0, 0.0, 0.0),
                ],
                waypoint_names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                speed_mps: 1.0,
                wait_ms: 100.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
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
    fn near_zero_ping_pong_segment_completes_without_spinning() {
        let mover = KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![
                    Vec3::ZERO,
                    Vec3::new(KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH * 0.5, 0.0, 0.0),
                ],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
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
