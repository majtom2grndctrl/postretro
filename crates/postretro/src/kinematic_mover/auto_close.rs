// Host-only automatic return timers for kinematic movers.
// See: context/lib/networking.md §Phase boundaries

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use postretro_entities::{EntityId, EntityRegistry, KinematicMoverComponent, MoverCommand};

use super::{MoverEventKind, mover_is_at_open_terminus, travel_toward_closed_terminus};

#[derive(Debug, Clone, Copy)]
struct AutoCloseCountdown {
    remaining_ms: f32,
    skip_next_tick: bool,
}

impl AutoCloseCountdown {
    fn armed(duration_ms: f32) -> Self {
        Self {
            remaining_ms: duration_ms,
            skip_next_tick: true,
        }
    }
}

#[derive(Debug, Default)]
struct AutoCloseTimerState {
    countdowns: HashMap<EntityId, AutoCloseCountdown>,
    enabled: bool,
}

/// Cloneable session-owned handle shared by the host command funnels and the
/// fixed-tick host simulation seam. The `Rc<RefCell<_>>` is main-thread only,
/// matching the existing command-diagnostics registration handles.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoverAutoCloseTimers {
    state: Rc<RefCell<AutoCloseTimerState>>,
}

impl MoverAutoCloseTimers {
    /// Enable only for the host/single-player session. Connected clients retain
    /// no active observer even though their long-lived command registries share
    /// the same construction path.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        state.enabled = enabled;
        if !enabled {
            state.countdowns.clear();
        }
    }

    /// Drop all transient level state without changing the session role.
    pub(crate) fn clear(&self) {
        self.state.borrow_mut().countdowns.clear();
    }

    /// Observe a command after the deterministic applier has run. A re-trigger
    /// only resets an already-open hold; it cannot manufacture an auto-close
    /// countdown for a mover which has not reached its open terminus.
    pub(crate) fn observe_command(
        &self,
        entity: EntityId,
        mover: &KinematicMoverComponent,
        command: &MoverCommand,
    ) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return;
        }
        match command {
            MoverCommand::Start | MoverCommand::GoToPathNode(_) => {
                if state.countdowns.contains_key(&entity) {
                    state
                        .countdowns
                        .insert(entity, AutoCloseCountdown::armed(mover.auto_close_ms));
                }
            }
            MoverCommand::Stop => {
                state.countdowns.remove(&entity);
            }
            MoverCommand::Reverse
            | MoverCommand::SetSpinRate(_)
            | MoverCommand::SetBlockPolicy(_) => {}
        }
    }

    /// Start timers from the already-host-only opened transition detector. A
    /// ping-pong mover otherwise reverses in its shared driver at the endpoint,
    /// so the host holds it there until expiry; a once mover is already held by
    /// its normal completed phase.
    pub(crate) fn arm_opened_termini(
        &self,
        registry: &mut EntityRegistry,
        events: &[(MoverEventKind, u32)],
    ) {
        if !self.state.borrow().enabled {
            return;
        }

        let opened_ids: Vec<u32> = events
            .iter()
            .filter_map(|(kind, mover_id)| (*kind == MoverEventKind::Opened).then_some(*mover_id))
            .collect();
        if opened_ids.is_empty() {
            return;
        }

        let movers: Vec<(EntityId, KinematicMoverComponent)> = registry
            .iter_with_kind(postretro_entities::ComponentKind::KinematicMover)
            .filter_map(|(entity, value)| {
                let postretro_entities::ComponentValue::KinematicMover(mover) = value else {
                    return None;
                };
                opened_ids
                    .contains(&mover.mover_id)
                    .then(|| (entity, mover.clone()))
            })
            .collect();

        for (entity, mut mover) in movers {
            if mover.auto_close_ms <= 0.0 || !mover.auto_close_ms.is_finite() {
                continue;
            }
            // Arrival provenance may contain an earlier open crossing even
            // when a fast ping-pong tick finishes back inside the path. Only
            // terminal phase can be converted into a host hold coherently.
            if !mover_is_at_open_terminus(&mover) {
                continue;
            }
            self.state
                .borrow_mut()
                .countdowns
                .insert(entity, AutoCloseCountdown::armed(mover.auto_close_ms));
            if mover.mode == postretro_entities::KinematicMoverMode::PingPong {
                mover.started = false;
                // Reuse the command applier's completed-hold edge: a Start
                // re-trigger remains phase-idempotent while this countdown is
                // active, and only resets the timer through the observer.
                mover.completed = true;
                mover.wait_remaining_ms = 0.0;
                mover.target_segment = None;
                let _ = registry.set_component(entity, mover);
            }
        }
    }

    /// Advance all active host timers immediately before the host blocking
    /// decision. If both select a phase mutation on this tick, the blocking pass
    /// runs afterward and therefore wins by design.
    pub(crate) fn tick(&self, registry: &mut EntityRegistry, tick_dt: f32) {
        let elapsed_ms = if tick_dt.is_finite() && tick_dt > 0.0 {
            tick_dt * 1000.0
        } else {
            return;
        };
        let expired: Vec<EntityId> = {
            let mut state = self.state.borrow_mut();
            if !state.enabled {
                return;
            }
            let mut expired = Vec::new();
            state.countdowns.retain(|entity, countdown| {
                if countdown.skip_next_tick {
                    countdown.skip_next_tick = false;
                    return true;
                }
                countdown.remaining_ms -= elapsed_ms;
                let active = countdown.remaining_ms > 0.0;
                if !active {
                    expired.push(*entity);
                }
                active
            });
            expired
        };

        for entity in expired {
            let Ok(mut mover) = registry
                .get_component::<KinematicMoverComponent>(entity)
                .cloned()
            else {
                continue;
            };
            if mover.waypoints.len() < 2 {
                continue;
            }
            travel_toward_closed_terminus(&mut mover);
            let _ = registry.set_component(entity, mover);
        }
    }

    #[cfg(test)]
    pub(crate) fn remaining_ms(&self, entity: EntityId) -> Option<f32> {
        self.state
            .borrow()
            .countdowns
            .get(&entity)
            .map(|countdown| countdown.remaining_ms)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use postretro_entities::{KinematicMoverConfig, KinematicMoverMode, Transform};

    use super::*;

    fn sample_mover() -> KinematicMoverComponent {
        let mut mover = KinematicMoverComponent::new(
            7,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["closed".to_string(), "open".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        mover.auto_close_ms = 100.0;
        mover.segment_index = 1;
        mover.completed = true;
        mover
    }

    #[test]
    fn opened_timer_retrigger_resets_and_stop_cancels() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        registry.set_component(entity, sample_mover()).unwrap();
        let timers = MoverAutoCloseTimers::default();
        timers.set_enabled(true);

        timers.arm_opened_termini(&mut registry, &[(MoverEventKind::Opened, 7)]);
        timers.tick(&mut registry, 0.075);
        assert_eq!(timers.remaining_ms(entity), Some(100.0));
        timers.tick(&mut registry, 0.075);
        assert!(timers.remaining_ms(entity).unwrap() < 30.0);

        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap()
            .clone();
        timers.observe_command(entity, &mover, &MoverCommand::Start);
        assert_eq!(timers.remaining_ms(entity), Some(100.0));

        timers.tick(&mut registry, 0.075);
        assert_eq!(timers.remaining_ms(entity), Some(100.0));
        timers.tick(&mut registry, 0.075);
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(entity)
                .unwrap()
                .completed
        );

        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap()
            .clone();
        timers.observe_command(entity, &mover, &MoverCommand::Stop);
        assert_eq!(timers.remaining_ms(entity), None);
    }

    #[test]
    fn expired_timer_travels_toward_the_closed_terminus() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        registry.set_component(entity, sample_mover()).unwrap();
        let timers = MoverAutoCloseTimers::default();
        timers.set_enabled(true);
        timers.arm_opened_termini(&mut registry, &[(MoverEventKind::Opened, 7)]);

        timers.tick(&mut registry, 0.1);
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(entity)
                .unwrap()
                .completed
        );
        timers.tick(&mut registry, 0.1);

        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap();
        assert!(mover.started);
        assert!(!mover.completed);
        assert_eq!(mover.direction_sign, -1);
        assert_eq!(mover.target_segment, Some(0));
    }

    // Regression: endpoint provenance from an earlier leg could snap an
    // interior ping-pong pose back to the open terminus when arming the timer.
    #[test]
    fn opened_arrival_does_not_arm_after_the_tick_finishes_interior() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform {
            position: Vec3::new(0.5, 0.0, 0.0),
            ..Transform::default()
        });
        let mut mover = sample_mover();
        mover.mode = KinematicMoverMode::PingPong;
        mover.completed = false;
        mover.started = true;
        mover.segment_index = 1;
        mover.direction_sign = -1;
        mover.segment_elapsed_ms = 500.0;
        registry.set_component(entity, mover).unwrap();
        let timers = MoverAutoCloseTimers::default();
        timers.set_enabled(true);

        timers.arm_opened_termini(&mut registry, &[(MoverEventKind::Opened, 7)]);

        assert_eq!(timers.remaining_ms(entity), None);
        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap();
        assert!(!mover.completed);
        assert_eq!(mover.direction_sign, -1);
        assert!((mover.segment_elapsed_ms - 500.0).abs() < f32::EPSILON);
        assert_eq!(
            registry
                .get_component::<Transform>(entity)
                .unwrap()
                .position,
            Vec3::new(0.5, 0.0, 0.0)
        );
    }

    // Regression: endpoint provenance must describe crossings without letting
    // the timer rewrite a different final phase or pose.
    #[test]
    fn fast_ping_pong_arms_only_when_multi_endpoint_tick_finishes_open() {
        let timers = MoverAutoCloseTimers::default();
        timers.set_enabled(true);

        let mut interior_registry = EntityRegistry::new();
        let interior_entity = interior_registry.spawn(Transform::default());
        let mut interior_mover = sample_mover();
        interior_mover.mode = KinematicMoverMode::PingPong;
        interior_mover.completed = false;
        interior_mover.segment_index = 0;
        interior_mover.direction_sign = 1;
        interior_registry
            .set_component(interior_entity, interior_mover)
            .unwrap();
        let mut interior_states = super::super::MoverTickStateTable::default();
        super::super::run_kinematic_mover_tick(&mut interior_registry, &mut interior_states, 1.5);
        let interior_events: Vec<_> = interior_states.terminus_events().collect();
        timers.arm_opened_termini(&mut interior_registry, &interior_events);
        assert_eq!(timers.remaining_ms(interior_entity), None);
        assert_eq!(
            interior_registry
                .get_component::<Transform>(interior_entity)
                .unwrap()
                .position,
            Vec3::new(0.5, 0.0, 0.0)
        );

        let mut open_registry = EntityRegistry::new();
        let open_entity = open_registry.spawn(Transform::default());
        let mut open_mover = sample_mover();
        open_mover.mode = KinematicMoverMode::PingPong;
        open_mover.completed = false;
        open_mover.segment_index = 0;
        open_mover.direction_sign = 1;
        open_registry
            .set_component(open_entity, open_mover)
            .unwrap();
        let mut open_states = super::super::MoverTickStateTable::default();
        super::super::run_kinematic_mover_tick(&mut open_registry, &mut open_states, 3.0);
        let open_events: Vec<_> = open_states.terminus_events().collect();
        timers.arm_opened_termini(&mut open_registry, &open_events);

        assert_eq!(timers.remaining_ms(open_entity), Some(100.0));
        assert!(
            open_registry
                .get_component::<KinematicMoverComponent>(open_entity)
                .unwrap()
                .completed
        );
        assert_eq!(
            open_registry
                .get_component::<Transform>(open_entity)
                .unwrap()
                .position,
            Vec3::X
        );
    }
}
