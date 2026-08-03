//! Host-only automatic return timers for kinematic movers.
//!
//! The mover phase remains the replicated source of truth. This side table only
//! decides when to issue a closeward directional intent, so connected clients
//! reconcile the resulting direction without ever observing or evaluating a
//! countdown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use postretro_entities::{EntityId, EntityRegistry, KinematicMoverComponent, MoverCommand};

use super::{MoverEventKind, travel_toward_closed_terminus};

#[derive(Debug, Default)]
struct AutoCloseTimerState {
    countdowns_ms: HashMap<EntityId, f32>,
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
            state.countdowns_ms.clear();
        }
    }

    /// Drop all transient level state without changing the session role.
    pub(crate) fn clear(&self) {
        self.state.borrow_mut().countdowns_ms.clear();
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
                if state.countdowns_ms.contains_key(&entity) {
                    state.countdowns_ms.insert(entity, mover.auto_close_ms);
                }
            }
            MoverCommand::Stop => {
                state.countdowns_ms.remove(&entity);
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
            self.state
                .borrow_mut()
                .countdowns_ms
                .insert(entity, mover.auto_close_ms);
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
            state.countdowns_ms.retain(|entity, remaining_ms| {
                *remaining_ms -= elapsed_ms;
                let active = *remaining_ms > 0.0;
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
        self.state.borrow().countdowns_ms.get(&entity).copied()
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
        assert!(timers.remaining_ms(entity).unwrap() < 30.0);

        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap()
            .clone();
        timers.observe_command(entity, &mover, &MoverCommand::Start);
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

        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap();
        assert!(mover.started);
        assert!(!mover.completed);
        assert_eq!(mover.direction_sign, -1);
        assert_eq!(mover.target_segment, Some(0));
    }
}
