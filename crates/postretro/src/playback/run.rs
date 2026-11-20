// Camera-route playback's run lifecycle: which pose each frame draws, which
// frames write rows, and when the route advances, ends or aborts. Pure: no
// App, window or GPU; time is passed in.
// See: context/lib/rendering_pipeline.md §12

use std::time::Instant;

use super::pacing;

/// Where a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Before the session's first gameplay level installs.
    AwaitingInstall,
    /// The level is installed and held; Settling streams from pose 0 and no
    /// row is written.
    Settling,
    /// `presented` hold frames have drawn pose 0 so far.
    Hold { presented: u32 },
    /// `frame` route frames have presented so far.
    Route { frame: usize },
    /// The last pose's route frame presented.
    Complete,
    /// A level request, a join or the stall timeout ended the run.
    Aborted,
}

/// What a row records a frame as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RowKind {
    Hold,
    Route,
    /// Streamed and drained from its pose, then presented no world.
    NoDraw,
}

/// How route frames choose their pose.
#[derive(Debug, Clone)]
pub(crate) enum Clock {
    /// Route frame k draws pose k.
    FrameLocked,
    /// A route frame draws the pose at its elapsed time since route frame 0.
    /// `cut[i]` marks pose `i` reached by a teleport.
    WallClock { cadence_hz: f64, cut: Box<[bool]> },
}

/// The pose one world frame draws, and what its row is if it presents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FramePlan {
    pub(crate) pose: usize,
    /// Wall-clock only: how far toward pose `pose + 1` the frame draws.
    pub(crate) blend: f32,
    pub(crate) kind: RowKind,
    /// Wall-clock route frames: seconds since route frame 0 began.
    pub(crate) route_time_s: Option<f64>,
    /// Presenting this frame ends the route.
    last: bool,
}

#[cfg(test)]
impl FramePlan {
    pub(crate) fn for_test(kind: RowKind, pose: usize) -> Self {
        Self {
            pose,
            blend: 0.0,
            kind,
            route_time_s: None,
            last: false,
        }
    }
}

/// What a frame that ran its plan did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameOutcome {
    /// The row to write: the plan of a frame that presented the world, a
    /// no-draw row for one that only streamed, none otherwise.
    pub(crate) row: Option<FramePlan>,
    /// That row was the route's last frame.
    pub(crate) complete: bool,
}

const NO_ROW: FrameOutcome = FrameOutcome {
    row: None,
    complete: false,
};

/// Hold frames 1..=N draw pose 0 (pin P1), then the route under its clock.
/// Only a frame that presented the world advances; one that streamed
/// without presenting writes a no-draw row and draws the same pose again
/// (pin P9).
#[derive(Debug, Clone)]
pub(crate) struct RouteRun {
    hold: u32,
    poses: usize,
    clock: Clock,
    phase: Phase,
    /// When route frame 0 began; wall-clock only.
    route_start: Option<Instant>,
    /// The plan `begin_world_frame` handed out this frame, until
    /// `finish_frame` takes it.
    current: Option<FramePlan>,
}

impl RouteRun {
    /// A run over `poses` poses (at least one) with `hold` hold frames.
    pub(crate) fn new(hold: u32, poses: usize, clock: Clock) -> Self {
        assert!(poses > 0, "a pose file holds at least one pose");
        if let Clock::WallClock { cut, .. } = &clock {
            assert_eq!(cut.len(), poses, "one cut flag per pose");
        }
        Self {
            hold,
            poses,
            clock,
            phase: Phase::AwaitingInstall,
            route_start: None,
            current: None,
        }
    }

    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    /// The first gameplay level installed and matched the route.
    pub(crate) fn arm(&mut self) {
        if self.phase == Phase::AwaitingInstall {
            self.phase = Phase::Settling;
        }
    }

    /// Armed and not ended: Settling, hold or route. Gameplay input stays
    /// suspended for all of it, and a level request aborts it.
    pub(crate) fn is_playing(&self) -> bool {
        matches!(
            self.phase,
            Phase::Settling | Phase::Hold { .. } | Phase::Route { .. }
        )
    }

    /// Past Settling and not ended: frames write rows, and a mid-run change
    /// marks them.
    pub(crate) fn writes_rows(&self) -> bool {
        matches!(self.phase, Phase::Hold { .. } | Phase::Route { .. })
    }

    /// The pose Settling holds.
    pub(crate) fn settling_pose(&self) -> Option<usize> {
        (self.phase == Phase::Settling).then_some(0)
    }

    /// Ends a playing run; false when it was not playing.
    pub(crate) fn abort(&mut self) -> bool {
        let playing = self.is_playing();
        if playing {
            self.phase = Phase::Aborted;
            self.current = None;
        }
        playing
    }

    /// The plan for a world frame beginning at `now`. The first one after
    /// Settling is the reveal frame: hold frame 1, or route frame 0 with a
    /// zero hold.
    pub(crate) fn begin_world_frame(&mut self, now: Instant) -> Option<FramePlan> {
        if self.phase == Phase::Settling {
            self.phase = self.after_hold(0);
        }
        self.current = match self.phase {
            Phase::Hold { .. } => Some(FramePlan {
                pose: 0,
                blend: 0.0,
                kind: RowKind::Hold,
                route_time_s: None,
                last: false,
            }),
            Phase::Route { frame } => Some(self.route_plan(frame, now)),
            Phase::AwaitingInstall | Phase::Settling | Phase::Complete | Phase::Aborted => None,
        };
        self.current
    }

    fn route_plan(&mut self, frame: usize, now: Instant) -> FramePlan {
        match &self.clock {
            Clock::FrameLocked => FramePlan {
                pose: frame,
                blend: 0.0,
                kind: RowKind::Route,
                route_time_s: None,
                last: frame + 1 == self.poses,
            },
            Clock::WallClock { cadence_hz, cut } => {
                let start = *self.route_start.get_or_insert(now);
                let elapsed = now.saturating_duration_since(start).as_secs_f64();
                let at = pacing::pick(elapsed, *cadence_hz, cut);
                FramePlan {
                    pose: at.pose,
                    blend: at.blend,
                    kind: RowKind::Route,
                    route_time_s: Some(elapsed),
                    last: at.last,
                }
            }
        }
    }

    /// The plan this frame is drawing, between begin and finish.
    pub(crate) fn current(&self) -> Option<FramePlan> {
        self.current
    }

    /// Ends the frame `begin_world_frame` planned. A frame that presented
    /// the world writes its row and advances. One that ran the streaming
    /// step but presented nothing writes a no-draw row and advances
    /// nothing; one that did neither writes nothing.
    pub(crate) fn finish_frame(&mut self, presented: bool, streamed: bool) -> FrameOutcome {
        let Some(plan) = self.current.take() else {
            return NO_ROW;
        };
        if !presented {
            // Route frame 0 is the first route frame that presents: its
            // clock starts again on the next one.
            if matches!(self.phase, Phase::Route { frame: 0 }) {
                self.route_start = None;
            }
            return FrameOutcome {
                row: streamed.then_some(FramePlan {
                    kind: RowKind::NoDraw,
                    last: false,
                    ..plan
                }),
                complete: false,
            };
        }
        self.phase = match self.phase {
            Phase::Hold { presented } => self.after_hold(presented + 1),
            Phase::Route { .. } if plan.last => Phase::Complete,
            Phase::Route { frame } => Phase::Route { frame: frame + 1 },
            other => other,
        };
        FrameOutcome {
            row: Some(plan),
            complete: self.phase == Phase::Complete,
        }
    }

    fn after_hold(&self, presented: u32) -> Phase {
        if presented < self.hold {
            Phase::Hold { presented }
        } else {
            Phase::Route { frame: 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Drives a run from Settling with `frames[i]` saying whether world
    /// frame i (presented, streamed); returns each frame's (plan, outcome).
    fn drive(
        run: &mut RouteRun,
        frames: impl IntoIterator<Item = (bool, bool)>,
    ) -> Vec<(Option<FramePlan>, FrameOutcome)> {
        let now = Instant::now();
        frames
            .into_iter()
            .map(|(presented, streamed)| {
                let plan = run.begin_world_frame(now);
                (plan, run.finish_frame(presented, streamed))
            })
            .collect()
    }

    fn presented(count: usize) -> impl Iterator<Item = (bool, bool)> {
        std::iter::repeat_n((true, true), count)
    }

    fn rows(frames: &[(Option<FramePlan>, FrameOutcome)]) -> Vec<(RowKind, usize)> {
        frames
            .iter()
            .filter_map(|(_, outcome)| outcome.row)
            .map(|row| (row.kind, row.pose))
            .collect()
    }

    fn armed(hold: u32, poses: usize) -> RouteRun {
        armed_with(hold, poses, Clock::FrameLocked)
    }

    fn armed_with(hold: u32, poses: usize, clock: Clock) -> RouteRun {
        let mut run = RouteRun::new(hold, poses, clock);
        assert_eq!(
            run.begin_world_frame(Instant::now()),
            None,
            "nothing plays before install"
        );
        run.arm();
        assert_eq!(run.settling_pose(), Some(0));
        assert!(!run.writes_rows(), "Settling writes no row");
        run
    }

    fn wall_clock(poses: usize) -> Clock {
        Clock::WallClock {
            cadence_hz: 60.0,
            cut: vec![false; poses].into_boxed_slice(),
        }
    }

    #[test]
    fn route_run_holds_pose_zero_then_draws_each_pose_once_as_a_route_frame() {
        // P1: hold N, then route frame k draws pose k.
        let (hold, poses) = (3, 4);
        let mut run = armed(hold, poses);
        let frames = drive(&mut run, presented(7));
        let rows = rows(&frames);
        assert_eq!(rows.len(), hold as usize + poses);
        assert!(
            rows[..3].iter().all(|row| *row == (RowKind::Hold, 0)),
            "the reveal frame is hold frame 1: {rows:?}"
        );
        assert_eq!(
            rows[3..].to_vec(),
            vec![
                (RowKind::Route, 0),
                (RowKind::Route, 1),
                (RowKind::Route, 2),
                (RowKind::Route, 3)
            ]
        );
        assert_eq!(
            rows.iter().filter(|row| row.1 == 0).count(),
            hold as usize + 1,
            "pose 0 draws N + 1 times, once as a route frame"
        );
        assert!(frames.last().unwrap().1.complete);
        assert_eq!(run.phase(), Phase::Complete);
        assert!(!run.is_playing());
        assert_eq!(
            run.begin_world_frame(Instant::now()),
            None,
            "a complete run plans nothing"
        );
    }

    #[test]
    fn route_run_one_pose_file_holds_then_draws_it_once_and_completes_in_either_pacing() {
        for clock in [Clock::FrameLocked, wall_clock(1)] {
            let mut run = armed_with(2, 1, clock);
            let frames = drive(&mut run, presented(3));
            assert_eq!(
                rows(&frames),
                vec![(RowKind::Hold, 0), (RowKind::Hold, 0), (RowKind::Route, 0)]
            );
            assert_eq!(
                frames.iter().map(|(_, o)| o.complete).collect::<Vec<_>>(),
                vec![false, false, true]
            );
        }
    }

    #[test]
    fn route_run_zero_hold_starts_the_route_on_the_reveal_frame() {
        let mut run = armed(0, 2);
        let frames = drive(&mut run, presented(2));
        assert_eq!(
            rows(&frames),
            vec![(RowKind::Route, 0), (RowKind::Route, 1)]
        );
        assert!(frames[1].1.complete);
    }

    #[test]
    fn route_run_no_draw_frames_write_a_row_and_advance_nothing() {
        // P9: a frame that streamed from pose k and got no surface writes a
        // no-draw row; the next world frame draws pose k again. A frame that
        // ran no streaming step writes nothing.
        let mut run = armed(1, 2);
        let frames = drive(
            &mut run,
            [
                (false, true),  // hold, no surface
                (true, true),   // hold 1
                (false, false), // route 0, no streaming step
                (false, true),  // route 0, no surface
                (true, true),   // route 0
                (false, true),  // route 1, no surface
                (true, true),   // route 1, last
            ],
        );
        let plans: Vec<(RowKind, usize)> = frames
            .iter()
            .filter_map(|(plan, _)| plan.map(|plan| (plan.kind, plan.pose)))
            .collect();
        assert_eq!(
            plans,
            vec![
                (RowKind::Hold, 0),
                (RowKind::Hold, 0),
                (RowKind::Route, 0),
                (RowKind::Route, 0),
                (RowKind::Route, 0),
                (RowKind::Route, 1),
                (RowKind::Route, 1),
            ],
            "an unpresented frame redraws its pose"
        );
        assert_eq!(
            rows(&frames),
            vec![
                (RowKind::NoDraw, 0),
                (RowKind::Hold, 0),
                (RowKind::NoDraw, 0),
                (RowKind::Route, 0),
                (RowKind::NoDraw, 1),
                (RowKind::Route, 1),
            ]
        );
        assert!(frames.last().unwrap().1.complete);
    }

    #[test]
    fn route_run_finish_without_a_plan_writes_nothing() {
        let mut run = armed(1, 1);
        assert_eq!(run.finish_frame(true, true), NO_ROW, "a Settling frame");
        assert_eq!(run.phase(), Phase::Settling);
        assert!(run.is_playing(), "Settling suspends gameplay input too");
    }

    #[test]
    fn route_run_aborts_only_while_playing() {
        let mut idle = RouteRun::new(0, 2, Clock::FrameLocked);
        assert!(!idle.abort(), "nothing to abort before install");
        assert_eq!(idle.phase(), Phase::AwaitingInstall);

        for presented_frames in [0, 1, 2] {
            let mut run = armed(1, 3);
            drive(&mut run, presented(presented_frames));
            assert!(run.abort(), "Settling, hold and route abort");
            assert_eq!(run.phase(), Phase::Aborted);
            assert!(!run.is_playing());
            assert_eq!(run.begin_world_frame(Instant::now()), None);
        }

        let mut done = armed(0, 1);
        drive(&mut done, presented(1));
        assert_eq!(done.phase(), Phase::Complete);
        assert!(!done.abort(), "a complete run stays complete (pin P2)");
    }

    #[test]
    fn route_run_wall_clock_draws_the_pose_at_elapsed_time_and_skips_between() {
        // P16: route frame 0 at elapsed 0 draws pose 0; a slowed run still
        // reaches pose k at k/60 s; the first frame past the last pose's
        // time draws it and ends the route.
        let mut run = armed_with(0, 10, wall_clock(10));
        let start = Instant::now();
        let mut drawn = Vec::new();
        for millis in [0_u64, 50, 117, 300] {
            let plan = run
                .begin_world_frame(start + Duration::from_millis(millis))
                .unwrap();
            drawn.push((plan.pose, plan.blend, plan.route_time_s.unwrap(), plan.last));
            let outcome = run.finish_frame(true, true);
            assert_eq!(outcome.complete, plan.last);
        }
        assert_eq!(drawn[0], (0, 0.0, 0.0, false));
        assert_eq!(drawn[1].0, 3, "50 ms is pose 3: poses 1 and 2 skipped");
        assert_eq!(drawn[2].0, 7, "117 ms is pose 7.02");
        assert!((drawn[1].2 - 0.05).abs() < 1.0e-9);
        assert_eq!(drawn[3], (9, 0.0, 0.3, true), "300 ms is past pose 9");
        assert_eq!(run.phase(), Phase::Complete);
    }

    #[test]
    fn route_run_wall_clock_starts_its_clock_at_the_first_presented_route_frame() {
        let mut run = armed_with(0, 4, wall_clock(4));
        let start = Instant::now();
        run.begin_world_frame(start);
        assert_eq!(
            run.finish_frame(false, true).row.unwrap().kind,
            RowKind::NoDraw
        );
        let plan = run
            .begin_world_frame(start + Duration::from_millis(40))
            .unwrap();
        assert_eq!(
            (plan.pose, plan.route_time_s),
            (0, Some(0.0)),
            "route frame 0 is the first that presents, at elapsed zero"
        );
    }
}
