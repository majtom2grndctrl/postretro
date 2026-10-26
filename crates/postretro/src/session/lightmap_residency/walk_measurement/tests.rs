//! Completed-reads mode, miss causes and the paired-run split (AC 41).
//! See: context/lib/testing_guide.md

use std::sync::Arc;

use postretro_visibility::{VisibilityPath, VisibleCells};

use super::LightmapStreamingSession;
use super::baked_set::cell_block_ranges;
use super::frame_driver::{COMPLETED_READS_DRAINS, FrameDriver, FrameInput, FrameOutcome};
use super::modes::ReadsMode;
use super::pool_mirror::{PoolMirror, RetireClock};
use super::trip_record::{self, FrameRecord, RunRecord};
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::lightmap_streaming::test_fixtures::{
    PORTAL, TestBlockSource, corridor_blocks, corridor_set,
};
use crate::session::level_streaming::LevelStreaming;
use crate::streaming::cell_demand::CellDemand;
use postretro_level_format::cell_residency_set::CellResidencySetSection;

/// Each 64×64 corridor block fills one 64² pool layer, and the cap is one
/// layer, so the camera's mandatory set grows the pool past its first
/// generation.
const LAYER_EDGE: u32 = 64;
const CAP_LAYERS: u32 = 1;
const STEP_LIMIT: VisibilityPath = VisibilityPath::PortalStepLimitFallback {
    considered: 0,
    accepted: 0,
};

/// The corridor level (cell c owns block c, default lead 16 m) streamed
/// through a real issuer into the pool mirror.
struct Corridor {
    driver: FrameDriver,
    set: CellResidencySetSection,
}

impl Corridor {
    fn new(reads: ReadsMode, clock: RetireClock) -> Self {
        let source = TestBlockSource::new(corridor_blocks(64, false));
        let set = corridor_set();
        let mut session = LightmapStreamingSession::with_source(
            Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
            &set,
            None,
        )
        .unwrap();
        session
            .controller
            .levers_mut()
            .set_pool_cap_layers(CAP_LAYERS);
        let mirror = PoolMirror::for_test_source(source.as_ref(), LAYER_EDGE, CAP_LAYERS, clock);
        let mut level = LevelStreaming::default();
        level.install_cell_demand(CellDemand::new(set.max_lead));
        level.install_lightmap(session);
        let cell_blocks = cell_block_ranges(0..7, 8);
        Self {
            driver: FrameDriver::new(level, mirror, reads, cell_blocks),
            set,
        }
    }

    fn frame(&mut self, camera_cell: u32, path: VisibilityPath, drawn: &[u32]) -> FrameOutcome {
        let visible = VisibleCells::Culled(drawn.to_vec());
        self.driver.frame(
            FrameInput {
                visible_cells: &visible,
                sh_visible_cells: &visible,
                camera_cell,
                path,
                seconds: 0.0,
            },
            &self.set,
        )
    }

    fn deferred_drains(&self) -> u64 {
        self.driver.mirror().stats.deferred
    }

    /// Frame 1 at camera 0 grows the pool for its mandatory blocks 0–2;
    /// frame 2 moves to camera 3, whose mandatory blocks 1–5 need room the
    /// retiring generation cannot give: 0 is evicted for 3, and 4 and 5 wait.
    fn grow_then_turn(&mut self) -> [FrameOutcome; 2] {
        let first = self.frame(0, PORTAL, &[0]);
        let second = self.frame(3, PORTAL, &[3, 4, 5]);
        [first, second]
    }
}

// AC 41 (O17, O29): completed reads land in the frame that issued them; a
// block deferred by both of a frame's drains counts once; retirement
// advances per frame, so a second drain does not release the generation.
#[test]
fn completed_reads_frame_counts_a_twice_deferred_block_once_and_retires_per_frame() {
    let mut corridor = Corridor::new(ReadsMode::CompletedReads, RetireClock::Frames);
    let [first, second] = corridor.grow_then_turn();
    assert_eq!(first.drains, COMPLETED_READS_DRAINS);
    assert!(
        first.grew,
        "camera 0's three mandatory blocks grow a one-layer pool"
    );
    assert_eq!(
        (first.deferred, first.left_behind),
        (0, 0),
        "frame 1's reads completed and installed before its misses were counted"
    );
    assert!(second.retiring_at_start);
    assert_eq!(
        (second.deferred, second.left_behind),
        (2, 0),
        "blocks 4 and 5 wait behind the retiring generation"
    );

    let before = corridor.deferred_drains();
    let third = corridor.frame(3, PORTAL, &[3, 4, 5]);
    assert_eq!(
        corridor.deferred_drains() - before,
        2 * u64::from(COMPLETED_READS_DRAINS),
        "both drains carried and deferred blocks 4 and 5"
    );
    assert_eq!(
        (third.deferred, third.left_behind),
        (2, 0),
        "each drawn block counts once per frame, however many drains defer it"
    );
    assert!(
        third.retiring_at_start && corridor.driver.mirror().retiring(),
        "four drains since the growth, but only two frames: still retiring"
    );

    let fourth = corridor.frame(3, PORTAL, &[3, 4, 5]);
    assert!(
        !fourth.retiring_at_start,
        "released at this frame's start, after two ticks on frames 2 and 3"
    );
    assert!(fourth.grew);
    assert_eq!((fourth.deferred, fourth.left_behind), (0, 0));
}

// The contrast that makes the frame clock matter: retiring per drain call,
// the same two-drain frames release the generation a frame early.
#[test]
fn per_drain_retirement_releases_a_frame_early_under_two_drains_per_frame() {
    let mut corridor = Corridor::new(ReadsMode::CompletedReads, RetireClock::Drains);
    let [_, second] = corridor.grow_then_turn();
    assert_eq!(second.deferred, 2);
    let third = corridor.frame(3, PORTAL, &[3, 4, 5]);
    assert!(
        third.grew,
        "the drain clock released the generation by frame 3"
    );
    assert_eq!(third.deferred, 0);
}

// AC 41 (O28, O29): a drawn block nothing demanded is left behind by
// admission, never deferred: a step-limit frame holds only resident draws
// and reads only the camera cell's baked set.
#[test]
fn completed_reads_frame_counts_an_undemanded_drawn_block_as_left_behind() {
    let mut corridor = Corridor::new(ReadsMode::CompletedReads, RetireClock::Frames);
    corridor.frame(0, PORTAL, &[0]);
    // Camera 0's baked set is cells 0–4; cell 6 is outside it, and on a
    // step-limit frame its block is neither read nor demanded.
    let trip = corridor.frame(0, STEP_LIMIT, &[0, 6]);
    assert_eq!(
        (trip.deferred, trip.left_behind),
        (0, 1),
        "block 6 is left behind; resident block 0 is no miss"
    );
}

/// A run's record from the fixture's frames, as the harness records them.
fn record(label: &str, frames: &[(VisibilityPath, FrameOutcome)]) -> RunRecord {
    RunRecord {
        label: label.to_string(),
        bake: "id51=fixture".to_string(),
        locate_mismatches: 0,
        frames: frames
            .iter()
            .enumerate()
            .map(|(i, (path, outcome))| FrameRecord::of(i as u64, i == 0, *path, outcome))
            .collect(),
    }
}

// AC 41 (O28, O38): a run records per frame whether its walk tripped, and a
// paired run on another build splits its frames by that record: the trip
// frame and the frames after it until a growth begun there has retired
// leave the gate, with both runs' totals reported apart.
#[test]
fn paired_run_splits_frames_by_the_recorded_runs_trips() {
    // Both runs stand at camera 0; the spawn frame's growth has retired by
    // frame 3. There the recorded (pre-change) run trips, so it reads only
    // the baked set while it draws cell 6. The rect run never trips: it
    // demands cell 6's block as visible and grows its pool for it.
    const TRIP_FRAME: usize = 3;
    let drawn = |frame: usize| {
        if frame == TRIP_FRAME {
            &[0, 6][..]
        } else {
            &[0][..]
        }
    };
    let mut pre = Corridor::new(ReadsMode::CompletedReads, RetireClock::Frames);
    let pre_frames: Vec<_> = (0..7)
        .map(|frame| {
            let path = if frame == TRIP_FRAME {
                STEP_LIMIT
            } else {
                PORTAL
            };
            (path, pre.frame(0, path, drawn(frame)))
        })
        .collect();
    let mut rect = Corridor::new(ReadsMode::CompletedReads, RetireClock::Frames);
    let rect_frames: Vec<_> = (0..7)
        .map(|frame| (PORTAL, rect.frame(0, PORTAL, drawn(frame))))
        .collect();
    assert!(
        rect_frames[TRIP_FRAME].1.grew && !pre_frames[TRIP_FRAME].1.grew,
        "only the rect run grows on the recorded trip frame"
    );

    let pre_record = record("corridor", &pre_frames);
    let rect_record = record("corridor", &rect_frames);
    let tripped: Vec<bool> = pre_record.frames.iter().map(|f| f.tripped).collect();
    assert_eq!(
        tripped,
        [false, false, false, true, false, false, false],
        "the record keeps each frame's trip"
    );

    // Through the file, as two builds would exchange it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trips.txt");
    let path = path.to_str().unwrap();
    trip_record::write_records(path, std::slice::from_ref(&pre_record)).unwrap();
    let read = trip_record::read_records(path).unwrap();
    assert_eq!(read, std::slice::from_ref(&pre_record));

    assert_eq!(
        trip_record::frames_apart(&read[0].frames, &rect_record.frames),
        [false, false, false, true, true, true, false],
        "the trip frame, then the two frames the rect run's growth retires over"
    );
    let split = trip_record::split(&read[0], &rect_record).unwrap();
    assert_eq!(split.spawn_frames, 1);
    assert_eq!((split.gate[0].frames, split.apart[0].frames), (3, 3));
    assert_eq!(split.held_after_trip, 2);
    assert_eq!(split.apart[1].trip_frames, 1);
    assert_eq!(
        split.apart[1].left_behind, 1,
        "the recorded run's undemanded draw on its trip frame is reported apart"
    );
    assert_eq!(
        (split.gate[0].not_resident(), split.gate[1].not_resident()),
        (0, 0)
    );
}

// O38: frame i must be the same pose in both runs; a pair whose poses or
// locate mismatches differ is invalid, not split.
#[test]
fn paired_run_with_a_different_pose_or_locate_count_is_invalid() {
    let frame = |fingerprint| FrameRecord {
        fingerprint,
        ..FrameRecord::default()
    };
    let recorded = RunRecord {
        label: "walk".to_string(),
        frames: vec![frame(1), frame(2)],
        ..RunRecord::default()
    };
    assert!(trip_record::split(&recorded, &recorded).is_ok());
    let moved = RunRecord {
        frames: vec![frame(1), frame(3)],
        ..recorded.clone()
    };
    assert!(trip_record::split(&recorded, &moved).is_err());
    let relocated = RunRecord {
        locate_mismatches: 1,
        ..recorded.clone()
    };
    assert!(trip_record::split(&recorded, &relocated).is_err());
    let other_modes = RunRecord {
        label: "walk at 130 deg".to_string(),
        ..recorded.clone()
    };
    assert!(trip_record::split(&recorded, &other_modes).is_err());
}
