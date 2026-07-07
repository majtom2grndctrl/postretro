//! Fixed-tick deterministic kinematic mover driver.
//!
//! The driver is intentionally a pure function of component phase, static path
//! data, and fixed `dt`. It reads no clock, RNG, host role, or external state.

use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverMode, Transform,
};

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
        if length <= f32::EPSILON {
            mover.segment_elapsed_ms = 0.0;
            mover.segment_index = to_index as u16;
            position = to;
            handle_arrival_at_waypoint(mover);
            continue;
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
    if !mover.started {
        return mover.waypoints[0];
    }
    if mover.completed {
        return *mover.waypoints.last().unwrap_or(&mover.waypoints[0]);
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
    if !length.is_finite() || length <= f32::EPSILON || mover.speed_mps <= 0.0 {
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
}
