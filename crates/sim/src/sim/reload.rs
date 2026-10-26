// Weapon reload timing, reserve transfer, feedback endpoints, and delivery events.
// See: context/lib/entity_model.md §5 · context/lib/ui.md §3

use postretro_entities::components::weapon::{ReloadFeedbackConsumer, WeaponComponent};
use postretro_entities::{EntityId, EntityRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadDelivery {
    #[cfg(feature = "test-support")]
    pub pawn: EntityId,
    #[cfg(not(feature = "test-support"))]
    pub(crate) pawn: EntityId,
    #[cfg(feature = "test-support")]
    pub weapon: EntityId,
    #[cfg(not(feature = "test-support"))]
    pub(crate) weapon: EntityId,
    pub outcome: ReloadOutcome,
}

impl ReloadDelivery {
    /// The pawn whose weapon reloaded: the event's emitter.
    pub fn pawn(&self) -> EntityId {
        self.pawn
    }

    /// The weapon instance that reloaded; its descriptor supplies the sounds.
    pub fn weapon(&self) -> EntityId {
        self.weapon
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    Started,
    Completed { transferred: u32 },
    BlockedFull,
    BlockedEmpty,
    ShellLoaded,
    Cancelled { transferred: u32 },
}

impl ReloadOutcome {
    pub const fn event_name(self) -> &'static str {
        match self {
            Self::Started => "reload_started",
            Self::Completed { .. } => "reload_completed",
            Self::BlockedFull => "reload_blocked_full",
            Self::BlockedEmpty => "reload_blocked_empty",
            Self::ShellLoaded => "reload_shell_loaded",
            Self::Cancelled { .. } => "reload_cancelled",
        }
    }
}

pub(super) fn tick_ms(tick_dt: f32) -> f64 {
    if tick_dt.is_finite() && tick_dt > 0.0 {
        f64::from(tick_dt) * 1000.0
    } else {
        0.0
    }
}

/// Treat an authored integer-millisecond boundary as reached when converting
/// the original `f32` tick to `f64` left it just below that same `f32` value.
/// The elapsed value itself stays unrounded so non-boundary carry remains exact.
pub(super) fn reaches_millisecond_boundary(elapsed_ms: f64, boundary_ms: u32) -> bool {
    let boundary = f64::from(boundary_ms);
    elapsed_ms >= boundary || elapsed_ms as f32 >= boundary_ms as f32
}

/// Whether this timed state will reach its current boundary on this tick, without
/// mutating its millisecond or fractional carry. Command ordering uses this to let
/// an already-due atomic reload complete before an accepted switch starts lowering.
pub(super) fn timer_expires_this_tick(component: &WeaponComponent, tick_dt: f32) -> bool {
    let carried_ms = if component.state_elapsed_sub_ms.is_finite()
        && component.state_elapsed_sub_ms >= 0.0
        && component.state_elapsed_sub_ms < 1.0
    {
        component.state_elapsed_sub_ms
    } else {
        0.0
    };
    reaches_millisecond_boundary(carried_ms + tick_ms(tick_dt), component.state_remaining_ms)
}

/// Advance the timed-state countdown and return its full millisecond overshoot
/// when it expires. The carry remains strictly sub-millisecond; callers that
/// restart a timed step apply the whole overshoot to that new step.
pub(super) fn advance_timer(component: &mut WeaponComponent, tick_dt: f32) -> Option<f64> {
    advance_countdown(
        &mut component.state_remaining_ms,
        &mut component.state_elapsed_sub_ms,
        tick_dt,
    )
}

/// Plain countdown shared by live state and equip-duration derivation. No weapon
/// descriptor or effective-stat graph is needed to step an equip timer.
fn advance_countdown(remaining_ms: &mut u32, sub_ms: &mut f64, tick_dt: f32) -> Option<f64> {
    let carried_ms = sub_ms
        .is_finite()
        .then_some(*sub_ms)
        .filter(|carried| (0.0..1.0).contains(carried))
        .unwrap_or(0.0);
    let elapsed_ms = carried_ms + tick_ms(tick_dt);
    if reaches_millisecond_boundary(elapsed_ms, *remaining_ms) {
        let overshoot_ms = (elapsed_ms - f64::from(*remaining_ms)).max(0.0);
        *remaining_ms = 0;
        *sub_ms = 0.0;
        return Some(overshoot_ms);
    }

    let whole_ms = elapsed_ms.floor() as u32;
    *remaining_ms -= whole_ms;
    *sub_ms = elapsed_ms - f64::from(whole_ms);
    None
}

/// Count the machine passes needed to expire a newly begun equip state. Even a
/// zero duration resolves on its first pass. Uses the live countdown's stepping
/// and fractional carry, rather than a separately rounded duration formula.
pub fn wieldable_equip_ticks(duration_ms: u32, tick_dt: f32) -> u32 {
    if duration_ms > 0 && tick_ms(tick_dt) == 0.0 {
        return u32::MAX;
    }
    let mut remaining_ms = duration_ms;
    let mut sub_ms = 0.0;
    let step_ms = tick_ms(tick_dt);
    let mut ticks = 0u32;

    // Common fixed ticks have at most 32 fractional binary digits. Adding their
    // carry to a step below 2^20 ms is exact in f64 (at most 52 binary digits).
    // Double exact countdown spans, then apply them only while safely before the
    // final step. The final boundary still uses advance_countdown, including its
    // f32 millisecond-boundary rule. No duration division or rounding is involved.
    // Unusual tick values retain literal stepping rather than approximate carry.
    if step_ms > 0.0 && step_ms < 1_048_576.0 && (step_ms * 4_294_967_296.0).fract() == 0.0 {
        let mut spans = [(0u64, 0.0f64); 32];
        spans[0] = (step_ms.floor() as u64, step_ms.fract());
        for index in 1..spans.len() {
            let (whole, fraction) = spans[index - 1];
            let doubled = fraction + fraction;
            spans[index] = (whole * 2 + doubled.floor() as u64, doubled.fract());
        }
        for index in (0..spans.len()).rev() {
            let (whole, fraction) = spans[index];
            let carried = sub_ms + fraction;
            let elapsed = whole + carried.floor() as u64;
            // Leaving more than a full step avoids skipping any canonical expiry,
            // including the slightly early f32 boundary accepted by that step.
            if elapsed < u64::from(remaining_ms)
                && f64::from(remaining_ms) - elapsed as f64 > step_ms.ceil()
            {
                remaining_ms -= elapsed as u32;
                sub_ms = carried.fract();
                ticks += 1u32 << index;
            }
        }
    }
    while ticks < u32::MAX {
        ticks += 1;
        if advance_countdown(&mut remaining_ms, &mut sub_ms, tick_dt).is_some() {
            return ticks;
        }
    }
    u32::MAX
}

/// Only an atomic reload may complete before a switch begin this tick.
pub fn wieldable_reload_due(component: &WeaponComponent, tick_dt: f32) -> bool {
    component.state == postretro_entities::components::wieldable_state::WieldableState::Reloading
        && timer_expires_this_tick(component, tick_dt)
}

/// Acknowledge only weapons actually sampled by owner-private projection.
pub fn clear_owner_feedback_for_weapons(registry: &mut EntityRegistry, ids: &[EntityId]) {
    for (index, &id) in ids.iter().enumerate() {
        if ids[..index].contains(&id) {
            continue;
        }
        clear_feedback_for_consumer(registry, id, ReloadFeedbackConsumer::OwnerProjection);
    }
}

pub fn clear_feedback_for_weapon(registry: &mut EntityRegistry, id: EntityId) {
    clear_feedback_for_consumer(registry, id, ReloadFeedbackConsumer::Hud);
}

fn clear_feedback_for_consumer(
    registry: &mut EntityRegistry,
    id: EntityId,
    consumer: ReloadFeedbackConsumer,
) {
    let Ok(mut weapon) = registry.get_component::<WeaponComponent>(id).cloned() else {
        return;
    };
    if weapon.acknowledge_reload_feedback(consumer) {
        let _ = registry.set_component(id, weapon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::data_descriptors::{ResolutionMode, WeaponDescriptor};

    // Regression: exact f32 tick durations widened just below integer millisecond boundaries.
    #[test]
    fn exact_f32_tick_durations_reach_integer_millisecond_boundaries() {
        for (remaining_ms, tick_dt) in [(10, 0.01), (20, 0.02)] {
            let mut component = WeaponComponent::from_descriptor(&WeaponDescriptor {
                sounds: None,
                knockback: None,
                damage: 0.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                bloom_per_shot_degrees: 0.0,
                bloom_max_degrees: 0.0,
                bloom_decay_degrees_per_second: 0.0,
                bloom_decay_delay_ms: 0.0,
                movement_spread_degrees: 0.0,
                spread_vertical_bias: 0.0,
                range: 0.0,
                primary: postretro_foundation::WeaponActivationDescriptor::single(
                    postretro_foundation::ActivationTrigger::Press,
                    0.0,
                ),
                secondary: None,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                splash: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            });
            component.state_remaining_ms = remaining_ms;
            component.state_total_ms = remaining_ms;

            let overshoot = advance_timer(&mut component, tick_dt)
                .expect("an exact tick duration expires the timer");
            assert!((overshoot - 0.0).abs() < f64::EPSILON);
            assert_eq!(component.state_remaining_ms, 0);
            assert!((component.state_elapsed_sub_ms - 0.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn equip_tick_spans_match_literal_countdown_steps() {
        let reference = |duration, dt| {
            let mut remaining = duration;
            let mut fraction = 0.0;
            for count in 1..=u32::MAX {
                if advance_countdown(&mut remaining, &mut fraction, dt).is_some() {
                    return count;
                }
            }
            u32::MAX
        };
        let mut random = 0x17ae_421du32;
        for dt in [1.0 / 60.0, 1.0 / 120.0, 0.01, 0.02, 0.1] {
            for duration in [0, 1, 9, 10, 16, 17, 20, 33, 34, 1000, 10001] {
                assert_eq!(wieldable_equip_ticks(duration, dt), reference(duration, dt));
            }
            for _ in 0..128 {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let duration = random % 100_000;
                assert_eq!(wieldable_equip_ticks(duration, dt), reference(duration, dt));
            }
        }
        // Large steps make literal stepping of the full authored u32 domain
        // affordable while exercising the same binary-span path.
        for duration in [u32::MAX, u32::MAX - 1, 1 << 31, (1 << 31) + 127] {
            assert_eq!(
                wieldable_equip_ticks(duration, 1000.0),
                reference(duration, 1000.0)
            );
        }
        assert_eq!(wieldable_equip_ticks(0, 0.0), 1);
        assert_eq!(wieldable_equip_ticks(1, 0.0), u32::MAX);
    }

    #[test]
    fn outcomes_map_to_stable_reaction_event_names() {
        assert_eq!(ReloadOutcome::Started.event_name(), "reload_started");
        assert_eq!(
            ReloadOutcome::Completed { transferred: 7 }.event_name(),
            "reload_completed"
        );
        assert_eq!(
            ReloadOutcome::BlockedFull.event_name(),
            "reload_blocked_full"
        );
        assert_eq!(
            ReloadOutcome::BlockedEmpty.event_name(),
            "reload_blocked_empty"
        );
        assert_eq!(
            ReloadOutcome::ShellLoaded.event_name(),
            "reload_shell_loaded"
        );
        assert_eq!(
            ReloadOutcome::Cancelled { transferred: 7 }.event_name(),
            "reload_cancelled"
        );
    }
}
