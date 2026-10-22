// Player jump presentation: the footing a movement state reports, and the jump
// sequence's phase memory advanced from it.
// See: context/lib/entity_model.md §7b

use postretro_entities::components::mesh::LocomotionJumpPhase;
use postretro_foundation::{MovementStateKind, PlayerMovementComponent};

use super::{LocomotionInput, LocomotionPosture};

const JUMP_UP_STATE: &str = "jump_up";
const JUMP_LOOP_STATE: &str = "jump_loop";
const JUMP_DOWN_STATE: &str = "jump_down";

/// Airborne ticks without a jump before the body leaves its ground stride for
/// `jump_loop`: 0.15 s at the 60 Hz fixed tick. Step-downs and ground-stick gaps
/// last a tick or two; a walk off a ledge outlasts this. Whole ticks, so the
/// threshold never rests on float rounding.
const FALL_DELAY_TICKS: u32 = 9;
/// Round-off allowance on the walking-pace landing test, so a pawn landing at
/// its capped walk speed still counts as walking.
const WALK_PACE_TOLERANCE: f32 = 1.0e-3;

/// Where a player's body is relative to the ground, as its movement state
/// reports it. Every field is replicated movement data, so each peer derives the
/// same footing for the same tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LocomotionFooting {
    #[default]
    Grounded,
    /// Off the ground in normal or crouched movement. `air_ticks` counts
    /// consecutive airborne ticks. `jumped` means the jump is spent: a jump fired
    /// this airborne stretch, or a teleport arrived into the air.
    Airborne {
        air_ticks: u32,
        vertical_speed: f32,
        jumped: bool,
    },
    /// Swimming: the water holds the body, so the jump sequence stays out of it,
    /// and a jump out of the water takes off as one from the ground does.
    Afloat,
    /// Vault, dash or slide: the movement state presents its own vertical
    /// motion, so the jump sequence stays out of it.
    Unsequenced,
}

impl LocomotionFooting {
    pub fn from_movement(
        kind: MovementStateKind,
        grounded: bool,
        air_ticks: u32,
        vertical_speed: f32,
        jump_spent: bool,
    ) -> Self {
        match kind {
            MovementStateKind::Normal | MovementStateKind::Crouch if grounded => Self::Grounded,
            MovementStateKind::Normal | MovementStateKind::Crouch => Self::Airborne {
                air_ticks,
                vertical_speed,
                jumped: jump_spent,
            },
            MovementStateKind::Swim => Self::Afloat,
            MovementStateKind::Dash | MovementStateKind::Slide | MovementStateKind::Vault => {
                Self::Unsequenced
            }
        }
    }

    fn air_ticks(self) -> u32 {
        match self {
            Self::Airborne { air_ticks, .. } => air_ticks,
            Self::Grounded | Self::Afloat | Self::Unsequenced => 0,
        }
    }

    fn rising_from_jump(self) -> bool {
        matches!(
            self,
            Self::Airborne { vertical_speed, jumped: true, .. } if vertical_speed > 0.0
        )
    }

    fn past_fall_delay(self) -> bool {
        matches!(self, Self::Airborne { air_ticks, .. } if air_ticks >= FALL_DELAY_TICKS)
    }
}

/// What a player's movement state says about its body's presentation: the
/// stance that picks the crouch tier and the footing that drives the jump
/// sequence. Remote peers read both from the replicated movement state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LocomotionStance {
    pub posture: LocomotionPosture,
    pub footing: LocomotionFooting,
}

impl LocomotionStance {
    pub fn from_movement(movement: &PlayerMovementComponent) -> Self {
        let kind = movement.movement_state.kind();
        Self {
            posture: LocomotionPosture::from_movement_state(kind),
            footing: LocomotionFooting::from_movement(
                kind,
                movement.is_grounded(),
                movement.air_ticks,
                movement.velocity.y,
                movement.jump_spent,
            ),
        }
    }
}

/// The jump sequence's declared states. Present only beside a declared
/// `jump_loop`; takeoff and landing each degrade to their neighbor when their
/// one-shot is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpStates {
    pub up: Option<&'static str>,
    pub airborne: &'static str,
    pub down: Option<&'static str>,
}

impl JumpStates {
    pub(super) fn from_declared(is_declared: impl Fn(&str) -> bool) -> Option<Self> {
        is_declared(JUMP_LOOP_STATE).then(|| Self {
            up: is_declared(JUMP_UP_STATE).then_some(JUMP_UP_STATE),
            airborne: JUMP_LOOP_STATE,
            down: is_declared(JUMP_DOWN_STATE).then_some(JUMP_DOWN_STATE),
        })
    }

    /// The one-shot states, whose clip lengths end them.
    pub(super) fn one_shots(self) -> impl Iterator<Item = &'static str> {
        self.up.into_iter().chain(self.down)
    }

    /// Advance the phase memory one selection. `now` is the animation clock,
    /// against which a one-shot's `since` stamp is compared with the clip length
    /// `clip_secs` reports; a one-shot whose length is unknown counts as played
    /// through. Airborne overrides posture: a crouched body jumps and falls like a
    /// standing one, and lands into its crouch stance without `jump_down`.
    ///
    /// A takeoff needs a fresh airborne stretch: once past the fall delay the body
    /// falls, so a spent jump still rising past it — an air dash or vault handing
    /// back mid-rise, a teleport arrival spending the jump — never replays
    /// `jump_up`. Only a jump out of the water takes off late, since the water held
    /// the body.
    ///
    /// A phase that last saw more airborne ticks than the footing reports missed
    /// a touchdown between two selections, as a presentation stepping past a
    /// one-tick bunny-hop landing does; it lands and takes off again as if it had
    /// seen the grounded tick. A peer that selects every tick never sees this.
    pub(super) fn advance(
        self,
        input: &LocomotionInput<'_>,
        now: f64,
        is_usable: impl Fn(&str) -> bool,
        clip_secs: impl Fn(&str) -> Option<f32>,
    ) -> LocomotionJumpPhase {
        use LocomotionJumpPhase::{Afloat, Falling, Grounded, Landing, Rising};
        if !is_usable(self.airborne) {
            return Grounded;
        }
        let up = self.up.filter(|state| is_usable(state));
        let down = self.down.filter(|state| is_usable(state));
        let playing = |state: Option<&str>, since: f64| {
            state
                .and_then(&clip_secs)
                .is_some_and(|secs| now - since < f64::from(secs))
        };
        let footing = input.footing;
        let air_ticks = footing.air_ticks();
        let falling = Falling { air_ticks };
        let rising = |since| {
            if playing(up, since) {
                Rising { since, air_ticks }
            } else {
                falling
            }
        };
        let landing = |since| {
            if input.posture == LocomotionPosture::Standing
                && at_walking_pace(input)
                && playing(down, since)
            {
                Landing { since }
            } else {
                Grounded
            }
        };
        // A missed touchdown lands first, as the unseen grounded tick would have.
        let previous = match (footing, input.jump_phase) {
            (
                LocomotionFooting::Airborne { air_ticks, .. },
                Rising {
                    air_ticks: seen, ..
                }
                | Falling { air_ticks: seen },
            ) if air_ticks < seen => landing(now),
            (_, phase) => phase,
        };
        match (footing, previous) {
            (LocomotionFooting::Afloat, _) => Afloat,
            (LocomotionFooting::Unsequenced, _) => Grounded,
            (LocomotionFooting::Airborne { .. }, Rising { since, .. }) => {
                if footing.rising_from_jump() {
                    rising(since)
                } else {
                    falling
                }
            }
            (LocomotionFooting::Airborne { .. }, Falling { .. }) => falling,
            // A jump out of the water; any other exit falls or keeps the stride.
            (_, Afloat) if footing.rising_from_jump() => rising(now),
            (_, Grounded | Landing { .. } | Afloat) if footing.past_fall_delay() => falling,
            // Taking off again cuts a landing short.
            (_, Grounded | Landing { .. }) if footing.rising_from_jump() => rising(now),
            // Touchdown. From here on the footing is grounded, or airborne too
            // briefly to leave the ground stride or a landing in progress.
            (_, Rising { .. } | Falling { .. }) => landing(now),
            (_, Landing { since }) => landing(since),
            (_, Grounded | Afloat) => Grounded,
        }
    }

    /// The phase a presentation with no history to advance from starts in, read
    /// from its footing alone. No one-shot starts: a body rising from a jump or
    /// past the fall delay is already in `jump_loop`, and a grounded one is not
    /// landing.
    pub(super) fn resync(
        self,
        footing: LocomotionFooting,
        is_usable: impl Fn(&str) -> bool,
    ) -> LocomotionJumpPhase {
        if !is_usable(self.airborne) {
            return LocomotionJumpPhase::Grounded;
        }
        match footing {
            LocomotionFooting::Afloat => LocomotionJumpPhase::Afloat,
            LocomotionFooting::Airborne { air_ticks, .. }
                if footing.rising_from_jump() || footing.past_fall_delay() =>
            {
                LocomotionJumpPhase::Falling { air_ticks }
            }
            _ => LocomotionJumpPhase::Grounded,
        }
    }

    /// Whether another peer's choice of `state` may play over `phase`. A one-shot
    /// is accepted only while this peer's own phase plays it, so a late
    /// correction never restarts one this peer has already left.
    pub(super) fn admits(self, state: &str, phase: LocomotionJumpPhase) -> bool {
        if self.up == Some(state) {
            matches!(phase, LocomotionJumpPhase::Rising { .. })
        } else if self.down == Some(state) {
            matches!(phase, LocomotionJumpPhase::Landing { .. })
        } else {
            true
        }
    }

    /// The state a phase plays, or `None` for the ground selection.
    pub(super) fn state_for(
        self,
        phase: LocomotionJumpPhase,
        is_usable: impl Fn(&str) -> bool,
    ) -> Option<&'static str> {
        if !is_usable(self.airborne) {
            return None;
        }
        match phase {
            LocomotionJumpPhase::Rising { .. } => Some(
                self.up
                    .filter(|state| is_usable(state))
                    .unwrap_or(self.airborne),
            ),
            LocomotionJumpPhase::Falling { .. } => Some(self.airborne),
            LocomotionJumpPhase::Landing { .. } => self.down.filter(|state| is_usable(state)),
            LocomotionJumpPhase::Grounded | LocomotionJumpPhase::Afloat => None,
        }
    }
}

fn at_walking_pace(input: &LocomotionInput<'_>) -> bool {
    input.speed_xz.is_finite() && input.speed_xz <= input.walk_speed + WALK_PACE_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::super::{LocomotionTierKind, PlayerLocomotionStates};
    use super::*;
    use LocomotionTierKind::{Idle, Jump, Run, Walk};
    use postretro_entities::components::mesh::LocomotionDirection;

    const TICK: f64 = 1.0 / 60.0;
    const DECLARED: [&str; 7] = [
        "walk_forward",
        "run_forward",
        "crouch_idle",
        "crouch_forward",
        "jump_up",
        "jump_loop",
        "jump_down",
    ];

    /// `jump_up` plays 0.4 s and `jump_down` 0.3 s.
    fn clip_secs(state: &str) -> Option<f32> {
        match state {
            "jump_up" => Some(0.4),
            "jump_down" => Some(0.3),
            _ => None,
        }
    }

    fn air(air_ticks: u32, vertical_speed: f32, jumped: bool) -> LocomotionFooting {
        LocomotionFooting::Airborne {
            air_ticks,
            vertical_speed,
            jumped,
        }
    }

    const GROUND: LocomotionFooting = LocomotionFooting::Grounded;

    /// One player's selections across ticks, carrying the state and jump memory
    /// the callers keep on its animation. Ground walk 7, run 11.
    struct Pawn<'a> {
        states: PlayerLocomotionStates,
        usable: &'a dyn Fn(&str) -> bool,
        clip_secs: &'a dyn Fn(&str) -> Option<f32>,
        phase: LocomotionJumpPhase,
        current: String,
    }

    impl<'a> Pawn<'a> {
        fn new(declared: &[&str], usable: &'a dyn Fn(&str) -> bool) -> Self {
            Self {
                states: PlayerLocomotionStates::from_declared(|state| declared.contains(&state))
                    .expect("walk state declared"),
                usable,
                clip_secs: &clip_secs,
                phase: LocomotionJumpPhase::Grounded,
                current: "idle".to_string(),
            }
        }

        fn step_in(
            &mut self,
            now: f64,
            speed: f32,
            posture: LocomotionPosture,
            footing: LocomotionFooting,
        ) -> (String, LocomotionTierKind) {
            let mut input = LocomotionInput {
                speed_xz: speed,
                walk_speed: 7.0,
                run_speed: 11.0,
                direction: LocomotionDirection::Forward,
                posture,
                footing,
                jump_phase: self.phase,
                current_state: &self.current,
            };
            self.phase = self
                .states
                .advance_jump_phase(&input, now, self.usable, self.clip_secs);
            input.jump_phase = self.phase;
            let (state, tier) = self.states.select(&input, self.usable);
            let state = state.unwrap_or("idle").to_string();
            self.current = state.clone();
            (state, tier)
        }

        fn step(
            &mut self,
            now: f64,
            speed: f32,
            footing: LocomotionFooting,
        ) -> (String, LocomotionTierKind) {
            self.step_in(now, speed, LocomotionPosture::Standing, footing)
        }

        /// Walk, jump, and fall past the apex into `jump_loop`.
        fn fall_from_a_jump(&mut self) {
            self.step(0.0, 3.0, GROUND);
            self.step(TICK, 3.0, air(1, 6.0, true));
            assert_eq!(self.step(0.3, 3.0, air(18, -1.0, true)).0, "jump_loop");
        }
    }

    fn s(state: &str, tier: LocomotionTierKind) -> (String, LocomotionTierKind) {
        (state.to_string(), tier)
    }

    const ALL_USABLE: &dyn Fn(&str) -> bool = &|_| true;

    #[test]
    fn jump_takeoff_selects_jump_up_until_the_apex() {
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        assert_eq!(pawn.step(0.0, 3.0, GROUND), s("walk_forward", Walk));
        assert_eq!(pawn.step(TICK, 3.0, air(1, 6.0, true)), s("jump_up", Jump));
        // Still rising, short of the clip's 0.4 s: not yet.
        assert_eq!(pawn.step(0.2, 3.0, air(12, 0.01, true)), s("jump_up", Jump));
        assert_eq!(
            pawn.step(0.21, 3.0, air(13, 0.0, true)),
            s("jump_loop", Jump)
        );
        // A falling body never returns to `jump_up`, even on an air jump.
        assert_eq!(
            pawn.step(0.25, 3.0, air(15, 6.0, true)),
            s("jump_loop", Jump)
        );
    }

    #[test]
    fn jump_up_hands_to_jump_loop_when_its_clip_ends_before_the_apex() {
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(1.0, 0.0, GROUND);
        assert_eq!(pawn.step(1.0 + TICK, 0.0, air(1, 9.0, true)).0, "jump_up");
        // `jump_up` entered at 1 + TICK and plays 0.4 s.
        assert_eq!(pawn.step(1.39, 0.0, air(23, 2.0, true)).0, "jump_up");
        assert_eq!(
            pawn.step(1.41 + TICK, 0.0, air(25, 1.5, true)).0,
            "jump_loop"
        );
    }

    #[test]
    fn walk_off_keeps_the_stride_until_the_fall_delay() {
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(0.0, 3.0, GROUND);
        for air_ticks in 1..=8 {
            let now = f64::from(air_ticks) * TICK;
            assert_eq!(
                pawn.step(now, 3.0, air(air_ticks, -0.3 * air_ticks as f32, false)),
                s("walk_forward", Walk),
                "{air_ticks} airborne ticks"
            );
        }
        // 9 ticks is 0.15 s.
        assert_eq!(
            pawn.step(9.0 * TICK, 3.0, air(9, -2.7, false)),
            s("jump_loop", Jump)
        );
        // A coyote jump inside the delay still takes off.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(0.0, 3.0, GROUND);
        pawn.step(TICK, 3.0, air(1, -0.3, false));
        assert_eq!(pawn.step(2.0 * TICK, 3.0, air(2, 6.0, true)).0, "jump_up");
    }

    #[test]
    fn brief_airborne_step_down_never_selects_a_jump_state() {
        for speed in [0.0, 3.0] {
            let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
            let ground = pawn.step(0.0, speed, GROUND);
            for (now, footing) in [
                (TICK, air(1, -0.5, false)),
                (2.0 * TICK, air(2, -0.8, false)),
                (3.0 * TICK, GROUND),
                (4.0 * TICK, air(1, -0.5, false)),
                (5.0 * TICK, GROUND),
            ] {
                assert_eq!(pawn.step(now, speed, footing), ground, "at {speed} m/s");
            }
        }
    }

    #[test]
    fn slow_landing_plays_jump_down_until_its_clip_ends() {
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(1.0, 0.0, GROUND), s("jump_down", Jump));
        // A ground-stick gap mid-landing keeps it.
        assert_eq!(
            pawn.step(1.1, 0.0, air(1, -0.2, false)),
            s("jump_down", Jump)
        );
        // `jump_down` entered at 1.0 and plays 0.3 s.
        assert_eq!(pawn.step(1.299, 0.0, GROUND), s("jump_down", Jump));
        assert_eq!(pawn.step(1.301, 0.0, GROUND), s("idle", Idle));
        assert_eq!(pawn.step(1.4, 3.0, GROUND), s("walk_forward", Walk));

        // Walking pace is inclusive.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(1.0, 7.0, GROUND), s("jump_down", Jump));
    }

    #[test]
    fn fast_landing_returns_straight_to_the_stride() {
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(1.0, 7.1, GROUND), s("walk_forward", Walk));
        // A bunny hop lands running: `jump_loop` is not run-tier memory, so the
        // run tier takes speed past its entry band.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(1.0, 11.0, GROUND), s("run_forward", Run));
    }

    #[test]
    fn jump_down_ends_early_on_speeding_past_walk_or_jumping_again() {
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(1.0, 2.0, GROUND).0, "jump_down");
        assert_eq!(pawn.step(1.05, 7.0, GROUND).0, "jump_down");
        assert_eq!(pawn.step(1.1, 7.2, GROUND), s("walk_forward", Walk));

        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(1.0, 0.0, GROUND).0, "jump_down");
        assert_eq!(pawn.step(1.1, 0.0, air(1, 6.0, true)), s("jump_up", Jump));
    }

    #[test]
    fn jump_fallbacks_degrade_per_state() {
        // No usable `jump_loop`: no sequence, airborne selects as on the ground.
        let unresolved_loop = |state: &str| state != "jump_loop";
        let no_loop = ["walk_forward", "jump_up", "jump_down"];
        for (declared, usable) in [
            (&DECLARED[..], &unresolved_loop as &dyn Fn(&str) -> bool),
            (&no_loop[..], ALL_USABLE),
        ] {
            let mut pawn = Pawn::new(declared, usable);
            assert_eq!(pawn.step(0.0, 3.0, air(1, 6.0, true)).0, "walk_forward");
            assert_eq!(pawn.step(0.5, 3.0, air(30, -3.0, true)).0, "walk_forward");
            assert_eq!(pawn.step(0.6, 0.0, GROUND).0, "idle");
            assert_eq!(pawn.phase, LocomotionJumpPhase::Grounded);
        }

        // No usable `jump_up`: takeoff goes straight to `jump_loop`.
        let unresolved_up = |state: &str| state != "jump_up";
        let no_up = ["walk_forward", "jump_loop", "jump_down"];
        for (declared, usable) in [
            (&DECLARED[..], &unresolved_up as &dyn Fn(&str) -> bool),
            (&no_up[..], ALL_USABLE),
        ] {
            let mut pawn = Pawn::new(declared, usable);
            pawn.step(0.0, 3.0, GROUND);
            assert_eq!(
                pawn.step(TICK, 3.0, air(1, 6.0, true)),
                s("jump_loop", Jump)
            );
            assert_eq!(pawn.step(1.0, 0.0, GROUND).0, "jump_down");
        }

        // No usable `jump_down`: landing goes straight to locomotion.
        let unresolved_down = |state: &str| state != "jump_down";
        let no_down = ["walk_forward", "jump_up", "jump_loop"];
        for (declared, usable) in [
            (&DECLARED[..], &unresolved_down as &dyn Fn(&str) -> bool),
            (&no_down[..], ALL_USABLE),
        ] {
            let mut pawn = Pawn::new(declared, usable);
            pawn.fall_from_a_jump();
            assert_eq!(pawn.step(1.0, 0.0, GROUND), s("idle", Idle));
        }

        // A one-shot whose clip length is unknown counts as played through.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.clip_secs = &|_| None;
        pawn.step(0.0, 0.0, GROUND);
        assert_eq!(pawn.step(TICK, 0.0, air(1, 6.0, true)).0, "jump_loop");
        assert_eq!(pawn.step(1.0, 0.0, GROUND).0, "idle");
    }

    #[test]
    fn unsequenced_movement_states_select_as_before_while_airborne() {
        for kind in [
            MovementStateKind::Dash,
            MovementStateKind::Slide,
            MovementStateKind::Swim,
            MovementStateKind::Vault,
        ] {
            let unsequenced = if kind == MovementStateKind::Swim {
                LocomotionFooting::Afloat
            } else {
                LocomotionFooting::Unsequenced
            };
            for grounded in [false, true] {
                assert_eq!(
                    LocomotionFooting::from_movement(kind, grounded, 30, 6.0, true),
                    unsequenced,
                    "{kind:?}"
                );
            }
            let posture = LocomotionPosture::from_movement_state(kind);
            let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
            let expected = if kind == MovementStateKind::Slide {
                s("crouch_idle", LocomotionTierKind::Slide)
            } else {
                s("walk_forward", Walk)
            };
            assert_eq!(pawn.step_in(0.0, 5.0, posture, unsequenced), expected);
            // Entering one mid-fall leaves the sequence: no landing afterward.
            let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
            pawn.fall_from_a_jump();
            assert_eq!(pawn.step_in(0.5, 5.0, posture, unsequenced), expected);
            assert_eq!(pawn.step(0.6, 0.0, GROUND), s("idle", Idle), "{kind:?}");
        }
    }

    #[test]
    fn crouched_body_jumps_and_falls_but_lands_into_its_crouch() {
        use LocomotionPosture::Crouching;
        for kind in [MovementStateKind::Normal, MovementStateKind::Crouch] {
            assert_eq!(
                LocomotionFooting::from_movement(kind, false, 4, 2.0, true),
                air(4, 2.0, true)
            );
            assert_eq!(
                LocomotionFooting::from_movement(kind, true, 0, 0.0, false),
                GROUND
            );
        }
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step_in(0.0, 0.0, Crouching, GROUND);
        assert_eq!(
            pawn.step_in(TICK, 0.0, Crouching, air(1, 6.0, true)),
            s("jump_up", Jump)
        );
        assert_eq!(
            pawn.step_in(0.3, 0.0, Crouching, air(18, -1.0, true)),
            s("jump_loop", Jump)
        );
        assert_eq!(
            pawn.step_in(0.6, 0.0, Crouching, GROUND),
            s("crouch_idle", Idle)
        );
    }

    #[test]
    fn jump_states_are_one_shots_or_airborne_never_rate_scaled() {
        let states = PlayerLocomotionStates::from_declared(|state| DECLARED.contains(&state))
            .expect("walk state declared");
        assert_eq!(
            states.one_shot_names().collect::<Vec<_>>(),
            ["jump_up", "jump_down"]
        );
        assert!(states.state_names().all(|name| !name.starts_with("jump")));
        for name in ["jump_up", "jump_loop", "jump_down"] {
            assert_eq!(states.direction_of(name, |_| true), None);
        }
    }

    // Regression: `jump_spent` outlives the airborne stretch, and an air dash
    // reset the phase to grounded, so the dash handing back mid-rise replayed
    // `jump_up` in mid-air.
    #[test]
    fn takeoff_needs_a_fresh_airborne_stretch() {
        let dash = LocomotionFooting::Unsequenced;
        // Jump, air dash with vertical speed preserved, dash ends still rising.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(0.0, 3.0, GROUND);
        assert_eq!(pawn.step(TICK, 3.0, air(1, 6.0, true)).0, "jump_up");
        for air_ticks in 2..=13 {
            pawn.step(f64::from(air_ticks) * TICK, 12.0, dash);
        }
        assert_eq!(
            pawn.step(14.0 * TICK, 3.0, air(14, 3.0, true)),
            s("jump_loop", Jump)
        );
        // A launch without a jump dashes, and a teleport arrival mid-dash spends
        // the jump: the dash hands back rising, but nothing took off.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(0.0, 3.0, GROUND);
        assert_eq!(pawn.step(TICK, 3.0, air(1, 8.0, false)).0, "walk_forward");
        for air_ticks in 2..=11 {
            pawn.step(f64::from(air_ticks) * TICK, 12.0, dash);
        }
        assert_eq!(
            pawn.step(12.0 * TICK, 3.0, air(12, 4.0, true)),
            s("jump_loop", Jump)
        );

        // The boundary is the fall delay: a spent jump rising at 8 airborne
        // ticks still takes off, at 9 it falls.
        for (air_ticks, expected) in [(8, "jump_up"), (9, "jump_loop")] {
            let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
            pawn.step(0.0, 3.0, dash);
            assert_eq!(
                pawn.step(TICK, 3.0, air(air_ticks, 3.0, true)).0,
                expected,
                "{air_ticks} airborne ticks"
            );
        }
        // A coyote jump inside the default 100 ms window takes off.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(0.0, 3.0, GROUND);
        for air_ticks in 1..=6 {
            pawn.step(
                f64::from(air_ticks) * TICK,
                3.0,
                air(air_ticks, -0.3, false),
            );
        }
        assert_eq!(pawn.step(7.0 * TICK, 3.0, air(7, 6.0, true)).0, "jump_up");
        // A jump out of the water takes off however long the swimmer has been
        // off the ground; leaving the water any other way does not.
        for (footing, expected) in [
            (air(240, 6.0, true), "jump_up"),
            (air(240, 2.0, false), "jump_loop"),
            (air(3, 2.0, false), "walk_forward"),
        ] {
            let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
            pawn.step(0.0, 3.0, LocomotionFooting::Afloat);
            assert_eq!(pawn.phase, LocomotionJumpPhase::Afloat);
            assert_eq!(pawn.step(TICK, 3.0, footing).0, expected, "{footing:?}");
        }
    }

    // Regression: a presentation stepping two ticks at a time skipped a bunny
    // hop's one grounded tick and kept falling through the next takeoff.
    #[test]
    fn missed_touchdown_lands_and_takes_off_again() {
        // A fall that sees a shorter airborne stretch has landed: a spent jump
        // rising again is a takeoff.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.phase, LocomotionJumpPhase::Falling { air_ticks: 18 });
        assert_eq!(pawn.step(0.5, 11.0, air(1, 6.0, true)), s("jump_up", Jump));
        assert_eq!(
            pawn.phase,
            LocomotionJumpPhase::Rising {
                since: 0.5,
                air_ticks: 1
            }
        );
        // A rise that sees one restarts `jump_up` from the new takeoff.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.step(0.0, 3.0, GROUND);
        pawn.step(TICK, 3.0, air(1, 6.0, true));
        assert_eq!(pawn.step(0.15, 3.0, air(9, 3.0, true)).0, "jump_up");
        assert_eq!(pawn.step(0.2, 3.0, air(2, 6.0, true)).0, "jump_up");
        assert!(
            matches!(pawn.phase, LocomotionJumpPhase::Rising { since, .. } if since == 0.2),
            "{:?}",
            pawn.phase
        );
        // Without a new jump the landing rule decides: a slow one plays
        // `jump_down` through the ground-stick gap, a fast one the stride.
        for (speed, expected) in [(0.0, s("jump_down", Jump)), (11.0, s("run_forward", Run))] {
            let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
            pawn.fall_from_a_jump();
            assert_eq!(pawn.step(0.5, speed, air(1, -0.2, false)), expected);
        }
        // An equal or longer stretch is the same fall.
        let mut pawn = Pawn::new(&DECLARED, ALL_USABLE);
        pawn.fall_from_a_jump();
        assert_eq!(pawn.step(0.31, 3.0, air(18, -1.2, true)).0, "jump_loop");
        assert_eq!(pawn.step(0.32, 3.0, air(19, 6.0, true)).0, "jump_loop");
    }

    #[test]
    fn resync_reads_the_phase_from_footing_without_a_one_shot() {
        use LocomotionJumpPhase::{Afloat, Falling, Grounded};
        let states = PlayerLocomotionStates::from_declared(|state| DECLARED.contains(&state))
            .expect("walk state declared");
        for (footing, expected) in [
            (GROUND, Grounded),
            (air(2, 6.0, true), Falling { air_ticks: 2 }),
            (air(8, -1.0, false), Grounded),
            (air(9, -1.0, false), Falling { air_ticks: 9 }),
            (LocomotionFooting::Afloat, Afloat),
            (LocomotionFooting::Unsequenced, Grounded),
        ] {
            assert_eq!(
                states.resync_jump_phase(footing, |_| true),
                expected,
                "{footing:?}"
            );
        }
        assert_eq!(
            states.resync_jump_phase(air(20, -1.0, false), |state| state != "jump_loop"),
            Grounded
        );
    }

    #[test]
    fn a_jump_one_shot_is_admitted_only_while_its_own_phase_plays_it() {
        use LocomotionJumpPhase::{Afloat, Falling, Grounded, Landing, Rising};
        let states = PlayerLocomotionStates::from_declared(|state| DECLARED.contains(&state))
            .expect("walk state declared");
        let rising = Rising {
            since: 0.0,
            air_ticks: 3,
        };
        let landing = Landing { since: 0.0 };
        for phase in [Grounded, rising, Falling { air_ticks: 3 }, landing, Afloat] {
            assert_eq!(states.admits_state("jump_up", phase), phase == rising);
            assert_eq!(states.admits_state("jump_down", phase), phase == landing);
            for state in ["jump_loop", "walk_forward", "idle"] {
                assert!(states.admits_state(state, phase), "{state} over {phase:?}");
            }
        }
    }
}
