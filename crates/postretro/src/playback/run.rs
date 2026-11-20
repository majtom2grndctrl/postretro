// Camera-route playback's run lifecycle: which pose each frame draws, and
// when the route advances and ends. Pure: no App, window or GPU.
// See: context/lib/rendering_pipeline.md §12

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
    /// The next route frame draws pose `frame`.
    Route { frame: usize },
    /// The last pose's route frame presented.
    Complete,
}

/// What a row records a frame as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RowKind {
    Hold,
    Route,
}

/// The pose one world frame draws, and what its row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePlan {
    pub(crate) pose: usize,
    pub(crate) kind: RowKind,
}

/// What a frame that ran its plan did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameOutcome {
    /// The row to write: the plan of a frame that presented the world, none
    /// for one that did not.
    pub(crate) row: Option<FramePlan>,
    /// That row was the last pose's route frame.
    pub(crate) complete: bool,
}

/// Frame-locked pacing (pin P1): hold frames 1..=N draw pose 0, then route
/// frame k draws pose k. A count advances only after a frame that
/// presented the world; one that did not draws the same pose again.
#[derive(Debug, Clone)]
pub(crate) struct RouteRun {
    hold: u32,
    poses: usize,
    phase: Phase,
    /// The plan `begin_world_frame` handed out this frame, until
    /// `finish_frame` takes it.
    current: Option<FramePlan>,
}

impl RouteRun {
    /// A run over `poses` poses (at least one) with `hold` hold frames.
    pub(crate) fn new(hold: u32, poses: usize) -> Self {
        assert!(poses > 0, "a pose file holds at least one pose");
        Self {
            hold,
            poses,
            phase: Phase::AwaitingInstall,
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

    /// Armed and not yet complete: Settling, hold or route. Gameplay input
    /// stays suspended for all of it.
    pub(crate) fn is_playing(&self) -> bool {
        matches!(
            self.phase,
            Phase::Settling | Phase::Hold { .. } | Phase::Route { .. }
        )
    }

    /// The pose Settling holds.
    pub(crate) fn settling_pose(&self) -> Option<usize> {
        (self.phase == Phase::Settling).then_some(0)
    }

    /// The plan for a world frame about to record. The first one after
    /// Settling is the reveal frame: hold frame 1, or route frame 0 with a
    /// zero hold.
    pub(crate) fn begin_world_frame(&mut self) -> Option<FramePlan> {
        if self.phase == Phase::Settling {
            self.phase = self.after_hold(0);
        }
        self.current = match self.phase {
            Phase::Hold { .. } => Some(FramePlan {
                pose: 0,
                kind: RowKind::Hold,
            }),
            Phase::Route { frame } => Some(FramePlan {
                pose: frame,
                kind: RowKind::Route,
            }),
            Phase::AwaitingInstall | Phase::Settling | Phase::Complete => None,
        };
        self.current
    }

    /// The plan this frame is drawing, between begin and finish.
    pub(crate) fn current(&self) -> Option<FramePlan> {
        self.current
    }

    /// Ends the frame `begin_world_frame` planned. Only a frame that
    /// presented the world writes a row and advances.
    pub(crate) fn finish_frame(&mut self, presented: bool) -> FrameOutcome {
        let Some(plan) = self.current.take() else {
            return FrameOutcome {
                row: None,
                complete: false,
            };
        };
        if !presented {
            return FrameOutcome {
                row: None,
                complete: false,
            };
        }
        self.phase = match self.phase {
            Phase::Hold { presented } => self.after_hold(presented + 1),
            Phase::Route { frame } if frame + 1 == self.poses => Phase::Complete,
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
    use super::*;

    /// Drives a run from Settling with `presented[i]` saying whether world
    /// frame i presented; returns each frame's (plan, row).
    fn drive(
        run: &mut RouteRun,
        presented: impl IntoIterator<Item = bool>,
    ) -> Vec<(Option<FramePlan>, FrameOutcome)> {
        presented
            .into_iter()
            .map(|presented| {
                let plan = run.begin_world_frame();
                (plan, run.finish_frame(presented))
            })
            .collect()
    }

    fn rows(frames: &[(Option<FramePlan>, FrameOutcome)]) -> Vec<FramePlan> {
        frames
            .iter()
            .filter_map(|(_, outcome)| outcome.row)
            .collect()
    }

    fn armed(hold: u32, poses: usize) -> RouteRun {
        let mut run = RouteRun::new(hold, poses);
        assert_eq!(
            run.begin_world_frame(),
            None,
            "nothing plays before install"
        );
        run.arm();
        assert_eq!(run.settling_pose(), Some(0));
        run
    }

    #[test]
    fn route_run_holds_pose_zero_then_draws_each_pose_once_as_a_route_frame() {
        // P1: hold N, then route frame k draws pose k.
        let (hold, poses) = (3, 4);
        let mut run = armed(hold, poses);
        let frames = drive(&mut run, std::iter::repeat_n(true, 7));
        let rows = rows(&frames);
        assert_eq!(rows.len(), hold as usize + poses);
        assert!(
            rows[..3].iter().all(|row| *row
                == FramePlan {
                    pose: 0,
                    kind: RowKind::Hold
                }),
            "the reveal frame is hold frame 1: {rows:?}"
        );
        let route: Vec<usize> = rows[3..]
            .iter()
            .inspect(|row| assert_eq!(row.kind, RowKind::Route))
            .map(|row| row.pose)
            .collect();
        assert_eq!(route, vec![0, 1, 2, 3]);
        assert_eq!(
            rows.iter().filter(|row| row.pose == 0).count(),
            hold as usize + 1,
            "pose 0 draws N + 1 times, once as a route frame"
        );
        assert!(frames.last().unwrap().1.complete);
        assert_eq!(run.phase(), Phase::Complete);
        assert!(!run.is_playing());
        assert_eq!(
            run.begin_world_frame(),
            None,
            "a complete run plans nothing"
        );
    }

    #[test]
    fn route_run_one_pose_file_holds_then_draws_it_once_and_completes() {
        let mut run = armed(2, 1);
        let frames = drive(&mut run, [true, true, true]);
        assert_eq!(
            rows(&frames),
            vec![
                FramePlan {
                    pose: 0,
                    kind: RowKind::Hold
                },
                FramePlan {
                    pose: 0,
                    kind: RowKind::Hold
                },
                FramePlan {
                    pose: 0,
                    kind: RowKind::Route
                },
            ]
        );
        assert_eq!(
            frames.iter().map(|(_, o)| o.complete).collect::<Vec<_>>(),
            vec![false, false, true]
        );
    }

    #[test]
    fn route_run_zero_hold_starts_the_route_on_the_reveal_frame() {
        let mut run = armed(0, 2);
        let frames = drive(&mut run, [true, true]);
        assert_eq!(
            rows(&frames),
            vec![
                FramePlan {
                    pose: 0,
                    kind: RowKind::Route
                },
                FramePlan {
                    pose: 1,
                    kind: RowKind::Route
                },
            ]
        );
        assert!(frames[1].1.complete);
    }

    #[test]
    fn route_run_frames_that_do_not_present_advance_nothing() {
        // P9's frame-locked half: the next world frame draws the same pose.
        let mut run = armed(1, 2);
        let frames = drive(&mut run, [false, true, false, false, true, false, true]);
        let plans: Vec<FramePlan> = frames.iter().filter_map(|(plan, _)| *plan).collect();
        assert_eq!(
            plans
                .iter()
                .map(|plan| (plan.kind, plan.pose))
                .collect::<Vec<_>>(),
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
        assert_eq!(rows(&frames).len(), 3, "only presented frames write rows");
        assert!(frames.last().unwrap().1.complete);
    }

    #[test]
    fn route_run_finish_without_a_plan_writes_nothing() {
        let mut run = armed(1, 1);
        assert_eq!(
            run.finish_frame(true),
            FrameOutcome {
                row: None,
                complete: false
            },
            "a Settling frame has no plan"
        );
        assert_eq!(run.phase(), Phase::Settling);
        assert!(run.is_playing(), "Settling suspends gameplay input too");
    }
}
