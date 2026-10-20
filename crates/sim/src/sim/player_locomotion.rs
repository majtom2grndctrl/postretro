// Player locomotion presentation: forward/backward classification, lower-body
// heading, and the engine-recognized locomotion state names.
// See: context/lib/entity_model.md §7b

use glam::Vec3;
use postretro_entities::components::mesh::{LocomotionDirection, MeshAnimation, MeshComponent};
use postretro_entities::{EntityId, EntityRegistry};

/// Travel farther than this from aim switches a forward-facing body to backward.
const BACKWARD_ENTER_ANGLE: f32 = 105.0_f32.to_radians();
/// Travel nearer than this to aim switches a backward-facing body to forward.
/// The band between the two holds the previous direction, so strafing while the
/// aim sweeps does not flip the legs 180° every time travel crosses 90°.
const BACKWARD_EXIT_ANGLE: f32 = 75.0_f32.to_radians();
const MIN_HORIZONTAL_LEN_SQ: f32 = 1e-8;

/// Walk-tier forward state names, in lookup order.
const WALK_FORWARD_STATES: [&str; 2] = ["walk_forward", "walk"];
const WALK_BACKWARD_STATE: &str = "walk_backward";
/// Run-tier forward state names, in lookup order.
const RUN_FORWARD_STATES: [&str; 2] = ["run_forward", "run"];
const RUN_BACKWARD_STATE: &str = "run_backward";

/// Classify a player's travel against its aim. Stationary or non-finite input
/// resets to `Forward`, so motion from a standstill starts from the nearer side.
pub fn classify_locomotion_direction(
    velocity: Vec3,
    aim_yaw: f32,
    previous: LocomotionDirection,
) -> LocomotionDirection {
    let Some(travel_yaw) = travel_yaw(velocity) else {
        return LocomotionDirection::Forward;
    };
    if !aim_yaw.is_finite() {
        return LocomotionDirection::Forward;
    }
    let off_aim = wrapped_angle(travel_yaw - aim_yaw).abs();
    if off_aim > BACKWARD_ENTER_ANGLE {
        LocomotionDirection::Backward
    } else if off_aim < BACKWARD_EXIT_ANGLE {
        LocomotionDirection::Forward
    } else {
        previous
    }
}

/// The yaw a player's lower body faces, in the engine's convention where yaw zero
/// faces `-Z`. Forward faces travel; backward faces against it, keeping the legs
/// toward aim while the body backpedals. Stationary or non-finite input returns
/// `fallback`.
pub fn player_travel_heading_yaw(
    velocity: Vec3,
    direction: LocomotionDirection,
    fallback: f32,
) -> f32 {
    match (travel_yaw(velocity), direction) {
        (Some(yaw), LocomotionDirection::Forward) => yaw,
        (Some(yaw), LocomotionDirection::Backward) => wrapped_angle(yaw + std::f32::consts::PI),
        (None, _) => fallback,
    }
}

/// Write the classification memory onto an animated mesh.
pub fn store_locomotion_direction(
    registry: &mut EntityRegistry,
    id: EntityId,
    direction: LocomotionDirection,
) {
    let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
        return;
    };
    let Some(animation) = mesh.animation.as_mut() else {
        return;
    };
    if animation.locomotion_direction != direction {
        animation.locomotion_direction = direction;
        let _ = registry.set_component(id, mesh);
    }
}

fn travel_yaw(velocity: Vec3) -> Option<f32> {
    let horizontal_len_sq = velocity.x * velocity.x + velocity.z * velocity.z;
    if !velocity.is_finite() || horizontal_len_sq <= MIN_HORIZONTAL_LEN_SQ {
        return None;
    }
    let yaw = (-velocity.x).atan2(-velocity.z);
    yaw.is_finite().then_some(yaw)
}

/// Shortest signed angle in [-π, π).
fn wrapped_angle(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (angle + PI).rem_euclid(TAU) - PI
}

/// Below this horizontal speed a player is still and plays its idle state.
const MOVING_SPEED_EPSILON: f32 = 1.0e-4;
/// Half-width of the walk/run hysteresis band, as a fraction of the gap between
/// the two movement speeds. Walking while turning carries ground speed above the
/// walk cap, so the tier switches at the midpoint rather than at walk speed, and
/// the band keeps speed noise near the midpoint from flickering the clip.
const RUN_TIER_BAND: f32 = 0.15;

/// One player's motion this tick, for [`PlayerLocomotionStates::select`].
#[derive(Debug, Clone, Copy)]
pub struct LocomotionInput<'a> {
    pub speed_xz: f32,
    pub walk_speed: f32,
    pub run_speed: f32,
    pub direction: LocomotionDirection,
    /// The active animation state: the tier hysteresis memory.
    pub current_state: &'a str,
}

/// Whether the run tier plays: speed is nearer run pace than walk pace, past a
/// band that a player already running must fall back through to walk again.
fn run_tier_holds(run: &LocomotionTier, input: &LocomotionInput<'_>) -> bool {
    let midpoint = (input.walk_speed + input.run_speed) * 0.5;
    let margin = (input.run_speed - input.walk_speed).abs() * RUN_TIER_BAND;
    let running =
        input.current_state == run.forward || run.backward.as_deref() == Some(input.current_state);
    if running {
        input.speed_xz >= midpoint - margin
    } else {
        input.speed_xz > midpoint + margin
    }
}

/// One speed tier's declared state names.
#[derive(Debug, Clone, PartialEq)]
pub struct LocomotionTier {
    pub forward: String,
    pub backward: Option<String>,
}

/// Which tier a selection landed in; callers pick the tier's reference speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocomotionTierKind {
    Idle,
    Walk,
    Run,
}

/// The engine-selected player locomotion states a descriptor declares. Names are
/// recognized by convention; a missing or unusable backward state falls back to
/// its tier's forward state, so the forward clip shows where a backward clip is
/// still owed.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerLocomotionStates {
    pub idle: String,
    pub walk: LocomotionTier,
    pub run: Option<LocomotionTier>,
}

impl PlayerLocomotionStates {
    /// `None` when no walk-forward state is declared: the engine then drives no
    /// locomotion states for this player.
    pub fn from_declared(idle: &str, is_declared: impl Fn(&str) -> bool) -> Option<Self> {
        let tier = |forward_names: [&str; 2], backward: &str| {
            let forward = forward_names.into_iter().find(|name| is_declared(name))?;
            Some(LocomotionTier {
                forward: forward.to_string(),
                backward: is_declared(backward).then(|| backward.to_string()),
            })
        };
        Some(Self {
            idle: idle.to_string(),
            walk: tier(WALK_FORWARD_STATES, WALK_BACKWARD_STATE)?,
            run: tier(RUN_FORWARD_STATES, RUN_BACKWARD_STATE),
        })
    }

    /// Read the declared states off a live animation.
    pub fn from_animation(animation: &MeshAnimation) -> Option<Self> {
        Self::from_declared(&animation.default_state, |state| {
            animation.states.contains_key(state)
        })
    }

    /// Every locomotion state name, for callers that precompute per-state data.
    pub fn state_names(&self) -> impl Iterator<Item = &str> {
        let tiers = std::iter::once(&self.walk).chain(self.run.as_ref());
        std::iter::once(self.idle.as_str()).chain(tiers.flat_map(|tier| {
            std::iter::once(tier.forward.as_str()).chain(tier.backward.as_deref())
        }))
    }

    /// Pick the target state. `is_usable` reports whether a state's clip
    /// resolved; an unusable backward state yields the tier's forward state.
    pub fn select(
        &self,
        input: &LocomotionInput<'_>,
        is_usable: impl Fn(&str) -> bool,
    ) -> (&str, LocomotionTierKind) {
        let speed = input.speed_xz;
        if !speed.is_finite() || speed <= MOVING_SPEED_EPSILON {
            return (&self.idle, LocomotionTierKind::Idle);
        }
        let (tier, kind) = match self.run.as_ref() {
            Some(run) if run_tier_holds(run, input) => (run, LocomotionTierKind::Run),
            _ => (&self.walk, LocomotionTierKind::Walk),
        };
        let backward = tier
            .backward
            .as_deref()
            .filter(|state| input.direction == LocomotionDirection::Backward && is_usable(state));
        (backward.unwrap_or(&tier.forward), kind)
    }

    /// The direction an entered state commits the body to, when it decides one: a
    /// backward state is backward, and a forward state whose tier has a usable
    /// backward state is forward. A forward state standing in for a missing
    /// backward clip decides nothing.
    pub fn direction_of(
        &self,
        state: &str,
        is_usable: impl Fn(&str) -> bool,
    ) -> Option<LocomotionDirection> {
        std::iter::once(&self.walk)
            .chain(self.run.as_ref())
            .find_map(|tier| {
                let backward = tier.backward.as_deref().filter(|name| is_usable(name))?;
                if state == backward {
                    Some(LocomotionDirection::Backward)
                } else if state == tier.forward {
                    Some(LocomotionDirection::Forward)
                } else {
                    None
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;
    use std::f32::consts::PI;

    const EPS: f32 = 1.0e-4;

    /// Velocity travelling `offset` radians from an aim of `aim_yaw`.
    fn travel(aim_yaw: f32, offset: f32) -> Vec3 {
        Quat::from_rotation_y(aim_yaw + offset) * Vec3::NEG_Z * 5.0
    }

    fn angle_diff(a: f32, b: f32) -> f32 {
        wrapped_angle(a - b).abs()
    }

    #[test]
    fn backpedal_keeps_legs_facing_aim() {
        let aim = 0.4;
        let velocity = travel(aim, PI);
        let direction = classify_locomotion_direction(velocity, aim, LocomotionDirection::Forward);
        assert_eq!(direction, LocomotionDirection::Backward);
        let heading = player_travel_heading_yaw(velocity, direction, aim);
        assert!(angle_diff(heading, aim) < EPS, "legs face aim, not travel");
    }

    #[test]
    fn forward_travel_faces_travel() {
        let aim = -1.2;
        let velocity = travel(aim, 0.5);
        let direction = classify_locomotion_direction(velocity, aim, LocomotionDirection::Forward);
        assert_eq!(direction, LocomotionDirection::Forward);
        let heading = player_travel_heading_yaw(velocity, direction, aim);
        assert!(angle_diff(heading, aim + 0.5) < EPS);
    }

    #[test]
    fn back_diagonal_twists_toward_the_front_diagonal() {
        let aim = 0.0;
        let velocity = travel(aim, 135.0_f32.to_radians());
        let direction = classify_locomotion_direction(velocity, aim, LocomotionDirection::Forward);
        assert_eq!(direction, LocomotionDirection::Backward);
        let heading = player_travel_heading_yaw(velocity, direction, aim);
        assert!(angle_diff(heading, -45.0_f32.to_radians()) < EPS);
    }

    #[test]
    fn dead_zone_holds_the_previous_direction_both_ways() {
        let aim = 2.0;
        for offset in [80.0_f32, 90.0, 100.0, -100.0] {
            let velocity = travel(aim, offset.to_radians());
            for previous in [LocomotionDirection::Forward, LocomotionDirection::Backward] {
                assert_eq!(
                    classify_locomotion_direction(velocity, aim, previous),
                    previous,
                    "{offset}° holds {previous:?}"
                );
            }
        }
    }

    #[test]
    fn dead_zone_edges_switch_direction() {
        let aim = -0.3;
        let enter = travel(aim, 106.0_f32.to_radians());
        assert_eq!(
            classify_locomotion_direction(enter, aim, LocomotionDirection::Forward),
            LocomotionDirection::Backward
        );
        let exit = travel(aim, -74.0_f32.to_radians());
        assert_eq!(
            classify_locomotion_direction(exit, aim, LocomotionDirection::Backward),
            LocomotionDirection::Forward
        );
    }

    #[test]
    fn classification_wraps_across_the_yaw_seam() {
        // Aim just shy of +π, travel just past -π: 2° apart, not 358°.
        let aim = PI - 0.01;
        let velocity = travel(aim, 0.035);
        assert_eq!(
            classify_locomotion_direction(velocity, aim, LocomotionDirection::Backward),
            LocomotionDirection::Forward
        );
        let behind = travel(aim, PI - 0.02);
        let direction = classify_locomotion_direction(behind, aim, LocomotionDirection::Forward);
        assert_eq!(direction, LocomotionDirection::Backward);
        let heading = player_travel_heading_yaw(behind, direction, aim);
        assert!((-PI..PI).contains(&heading));
        assert!(angle_diff(heading, aim) < 0.03);
    }

    #[test]
    fn stationary_or_non_finite_motion_falls_back() {
        for velocity in [
            Vec3::ZERO,
            Vec3::new(0.0, -3.0, 0.0),
            Vec3::new(f32::NAN, 0.0, 1.0),
        ] {
            assert_eq!(
                classify_locomotion_direction(velocity, 0.2, LocomotionDirection::Backward),
                LocomotionDirection::Forward
            );
            for direction in [LocomotionDirection::Forward, LocomotionDirection::Backward] {
                assert_eq!(player_travel_heading_yaw(velocity, direction, 0.7), 0.7);
            }
        }
        assert_eq!(
            classify_locomotion_direction(travel(0.0, PI), f32::NAN, LocomotionDirection::Backward),
            LocomotionDirection::Forward
        );
    }

    #[test]
    fn upper_body_twist_stays_bounded_for_every_travel_direction() {
        let bound = BACKWARD_ENTER_ANGLE + EPS;
        for aim_step in 0..8 {
            let aim = -PI + aim_step as f32 * 0.8;
            for previous in [LocomotionDirection::Forward, LocomotionDirection::Backward] {
                for step in 0..360 {
                    let velocity = travel(aim, (step as f32).to_radians());
                    let direction = classify_locomotion_direction(velocity, aim, previous);
                    let heading = player_travel_heading_yaw(velocity, direction, aim);
                    assert!(
                        angle_diff(aim, heading) <= bound,
                        "aim {aim} travel {step}° from {previous:?} twists {}",
                        angle_diff(aim, heading).to_degrees()
                    );
                }
            }
        }
    }

    fn states(declared: &[&str]) -> PlayerLocomotionStates {
        PlayerLocomotionStates::from_declared("idle", |state| declared.contains(&state))
            .expect("walk state declared")
    }

    /// Speed `speed` against ground walk 7 / run 11 (midpoint 9, band 8.4..9.6).
    fn input<'a>(
        speed: f32,
        direction: LocomotionDirection,
        current_state: &'a str,
    ) -> LocomotionInput<'a> {
        LocomotionInput {
            speed_xz: speed,
            walk_speed: 7.0,
            run_speed: 11.0,
            direction,
            current_state,
        }
    }

    #[test]
    fn selection_matrix_covers_tiers_directions_and_fallbacks() {
        use LocomotionDirection::{Backward, Forward};
        use LocomotionTierKind::{Idle, Run, Walk};
        let all = states(&[
            "walk_forward",
            "walk_backward",
            "run_forward",
            "run_backward",
        ]);
        let usable = |_: &str| true;
        let pick =
            |states: &PlayerLocomotionStates, speed, direction, usable: &dyn Fn(&str) -> bool| {
                let (state, tier) = states.select(&input(speed, direction, "idle"), usable);
                (state.to_string(), tier)
            };
        let s = |name: &str, tier| (name.to_string(), tier);
        assert_eq!(pick(&all, 0.0, Backward, &usable), s("idle", Idle));
        assert_eq!(pick(&all, 7.0, Forward, &usable), s("walk_forward", Walk));
        assert_eq!(pick(&all, 7.0, Backward, &usable), s("walk_backward", Walk));
        assert_eq!(pick(&all, 11.0, Forward, &usable), s("run_forward", Run));
        assert_eq!(pick(&all, 11.0, Backward, &usable), s("run_backward", Run));

        // Undeclared backward states fall back to their own tier's forward state.
        let forward_only = states(&["walk_forward", "run"]);
        assert_eq!(
            pick(&forward_only, 7.0, Backward, &usable),
            s("walk_forward", Walk)
        );
        assert_eq!(pick(&forward_only, 11.0, Backward, &usable), s("run", Run));

        // No cross-tier fallback: a run backpedal without `run_backward` stays
        // in the run tier even when `walk_backward` exists.
        let walk_backward_only = states(&["walk_forward", "walk_backward", "run_forward"]);
        assert_eq!(
            pick(&walk_backward_only, 11.0, Backward, &usable),
            s("run_forward", Run)
        );

        // A declared backward state whose clip did not resolve also falls back.
        let unresolved = |state: &str| state != "walk_backward";
        assert_eq!(
            pick(&all, 7.0, Backward, &unresolved),
            s("walk_forward", Walk)
        );

        // No run state declared: sprint speed stays in the walk tier.
        let walk_only = states(&["walk", "walk_backward"]);
        assert_eq!(pick(&walk_only, 11.0, Forward, &usable), s("walk", Walk));
        assert_eq!(
            pick(&walk_only, 11.0, Backward, &usable),
            s("walk_backward", Walk)
        );
    }

    #[test]
    fn run_tier_switches_at_the_midpoint_with_a_band_both_ways() {
        let all = states(&["walk_forward", "walk_backward", "run_forward", "run"]);
        let usable = |_: &str| true;
        let tier = |speed, current| {
            all.select(&input(speed, LocomotionDirection::Forward, current), usable)
                .1
        };
        // Walking while turning the mouse carries speed past the walk cap (about
        // 8.4 at 45°/s); that must stay walk.
        assert_eq!(tier(8.4, "walk_forward"), LocomotionTierKind::Walk);
        assert_eq!(tier(9.5, "walk_forward"), LocomotionTierKind::Walk);
        assert_eq!(tier(9.7, "walk_forward"), LocomotionTierKind::Run);
        assert_eq!(tier(9.7, "idle"), LocomotionTierKind::Run);
        // A running player holds the run tier down through the band.
        assert_eq!(tier(8.5, "run_forward"), LocomotionTierKind::Run);
        assert_eq!(tier(8.3, "run_forward"), LocomotionTierKind::Walk);
        // Any declared run-tier name is run-tier memory, `run_backward` too.
        let backward_run = states(&["walk_forward", "run_forward", "run_backward"]);
        assert_eq!(
            backward_run
                .select(
                    &input(8.5, LocomotionDirection::Backward, "run_backward"),
                    usable
                )
                .0,
            "run_backward"
        );
        // Equal walk and run speeds degenerate to the plain threshold.
        let flat = |speed| LocomotionInput {
            speed_xz: speed,
            walk_speed: 7.0,
            run_speed: 7.0,
            direction: LocomotionDirection::Forward,
            current_state: "walk_forward",
        };
        assert_eq!(all.select(&flat(7.0), usable).1, LocomotionTierKind::Walk);
        assert_eq!(all.select(&flat(7.1), usable).1, LocomotionTierKind::Run);
    }

    #[test]
    fn no_walk_state_means_no_engine_locomotion() {
        assert!(
            PlayerLocomotionStates::from_declared("idle", |state| state == "run_forward").is_none()
        );
    }

    #[test]
    fn entered_states_decide_direction_only_when_a_backward_clip_exists() {
        let all = states(&["walk_forward", "walk_backward", "run", "run_backward"]);
        let usable = |_: &str| true;
        assert_eq!(
            all.direction_of("walk_backward", usable),
            Some(LocomotionDirection::Backward)
        );
        assert_eq!(
            all.direction_of("run", usable),
            Some(LocomotionDirection::Forward)
        );
        assert_eq!(all.direction_of("idle", usable), None);
        // The forward state standing in for a missing backward clip decides nothing.
        let forward_only = states(&["walk_forward"]);
        assert_eq!(forward_only.direction_of("walk_forward", usable), None);
        let unresolved = |state: &str| state != "walk_backward";
        assert_eq!(all.direction_of("walk_forward", unresolved), None);
    }
}
