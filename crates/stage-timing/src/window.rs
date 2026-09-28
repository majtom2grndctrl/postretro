// Label-keyed fold of frame records into fixed-length windows.
// See: context/lib/rendering_pipeline.md §12

use crate::{FrameRecord, MAX_FRAME_SAMPLES, StageKind};

/// Frames per window. Matches GPU `FrameTiming`'s window length, though the
/// two never align: the GPU window counts completed readbacks.
pub const WINDOW_FRAMES: u32 = 120;

#[derive(Debug, Clone, Copy)]
struct Accumulator {
    label: &'static str,
    parent: Option<&'static str>,
    kind: StageKind,
    aggregate: bool,
    frames: u32,
    sum: u64,
    max: u64,
}

/// One stage's summary over a closed window. Present only if the stage ran in
/// at least one frame of it: absent is never reported as zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowRow {
    pub label: &'static str,
    pub parent: Option<&'static str>,
    pub kind: StageKind,
    /// A frame aggregate (total, work CPU) rather than a stage; see
    /// [`crate::Sample::aggregate`].
    pub aggregate: bool,
    /// Frames of the window this stage ran in.
    pub frames: u32,
    /// Mean over `frames` only (nanoseconds for time, units for counts).
    pub average: f64,
    pub max: u64,
}

impl WindowRow {
    pub fn average_ms(&self) -> f64 {
        self.average / 1.0e6
    }

    pub fn max_ms(&self) -> f64 {
        self.max as f64 / 1.0e6
    }
}

/// A closed window. Rows are in depth-first pre-order: each row is followed by
/// its substages, siblings keep the order they were first seen in, and a row
/// whose parent did not run in the window is a root.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WindowSnapshot {
    pub frames: u32,
    pub rows: Vec<WindowRow>,
}

impl WindowSnapshot {
    fn with_capacity() -> Self {
        Self {
            frames: 0,
            rows: Vec::with_capacity(MAX_FRAME_SAMPLES),
        }
    }

    pub fn row(&self, label: &str) -> Option<&WindowRow> {
        self.rows.iter().find(|row| row.label == label)
    }

    /// Depth of a row in the stage tree (0 for top-level).
    pub fn depth(&self, row: &WindowRow) -> usize {
        let mut depth = 0;
        let mut parent = row.parent;
        while let Some(label) = parent {
            depth += 1;
            parent = self.row(label).and_then(|row| row.parent);
            if depth > MAX_FRAME_SAMPLES {
                break;
            }
        }
        depth
    }

    /// Writes one `label=value` field per row, in row order. Time rows show
    /// `avg/max ms`; count rows show `avg/max`; marker rows show a frame count.
    /// Every row carries `(ran/frames)` when it ran in only part of the
    /// window. Surface formatting only — allocates through `out`.
    pub fn write_fields(&self, out: &mut impl std::fmt::Write) -> std::fmt::Result {
        for (index, row) in self.rows.iter().enumerate() {
            if index > 0 {
                out.write_char(' ')?;
            }
            match row.kind {
                StageKind::Time => write!(
                    out,
                    "{}={:.3}/{:.3}ms",
                    row.label,
                    row.average_ms(),
                    row.max_ms()
                )?,
                StageKind::Count => write!(out, "{}={:.1}/{}", row.label, row.average, row.max)?,
                StageKind::Marker => write!(out, "{}={}", row.label, row.frames)?,
            }
            if row.kind != StageKind::Marker && row.frames < self.frames {
                write!(out, "({}/{})", row.frames, self.frames)?;
            }
        }
        Ok(())
    }

    pub fn fields_string(&self) -> String {
        let mut out = String::new();
        let _ = self.write_fields(&mut out);
        out
    }
}

/// Folds [`FrameRecord`]s into 120-frame windows.
///
/// All row storage is allocated in [`StageWindow::new`]; folding and window
/// close reuse it. Construct one only when timing is on.
pub struct StageWindow {
    rows: Vec<Accumulator>,
    frames: u32,
    last: WindowSnapshot,
    has_last: bool,
    spare: WindowSnapshot,
    completed_unread: bool,
}

impl StageWindow {
    pub fn new() -> Self {
        Self {
            rows: Vec::with_capacity(MAX_FRAME_SAMPLES),
            frames: 0,
            last: WindowSnapshot::with_capacity(),
            has_last: false,
            spare: WindowSnapshot::with_capacity(),
            completed_unread: false,
        }
    }

    /// Adds one counted frame. Returns `true` when this frame closed a window.
    pub fn fold(&mut self, record: &FrameRecord) -> bool {
        for sample in record.samples() {
            let row = match self.rows.iter().position(|row| row.label == sample.label) {
                Some(index) => &mut self.rows[index],
                None => {
                    if self.rows.len() == self.rows.capacity() {
                        debug_assert!(false, "StageWindow row overflow at {}", sample.label);
                        continue;
                    }
                    self.rows.push(Accumulator {
                        label: sample.label,
                        parent: sample.parent,
                        kind: sample.kind,
                        aggregate: sample.aggregate,
                        frames: 0,
                        sum: 0,
                        max: 0,
                    });
                    self.rows.last_mut().expect("row just pushed")
                }
            };
            row.frames += 1;
            row.sum = row.sum.saturating_add(sample.value);
            row.max = row.max.max(sample.value);
        }
        self.frames += 1;
        if self.frames < WINDOW_FRAMES {
            return false;
        }
        self.close();
        true
    }

    fn close(&mut self) {
        let rows = &self.rows;
        let snapshot = &mut self.spare;
        snapshot.frames = self.frames;
        snapshot.rows.clear();
        let ran = |label: &str| rows.iter().any(|row| row.frames > 0 && row.label == label);
        for (index, row) in rows.iter().enumerate() {
            let is_root = row.parent.is_none_or(|parent| !ran(parent));
            if row.frames > 0 && is_root {
                push_subtree(rows, index, &mut snapshot.rows);
            }
        }
        std::mem::swap(&mut self.last, &mut self.spare);
        self.has_last = true;
        self.completed_unread = true;
        self.discard_partial();
    }

    /// Drops the frames folded since the last close. Rows keep their storage.
    pub fn discard_partial(&mut self) {
        self.frames = 0;
        for row in &mut self.rows {
            row.frames = 0;
            row.sum = 0;
            row.max = 0;
        }
    }

    /// Drops the partial window and the last closed one, so no surface shows
    /// a window measured before this call (a new level, for example).
    pub fn clear(&mut self) {
        self.discard_partial();
        self.has_last = false;
        self.completed_unread = false;
    }

    /// The latest closed window. Reading does not consume it.
    pub fn last_window(&self) -> Option<&WindowSnapshot> {
        self.has_last.then_some(&self.last)
    }

    /// The latest closed window, once per close. For consumers that must see
    /// each window exactly once (capture). Allocates the returned copy.
    pub fn take_completed_window(&mut self) -> Option<WindowSnapshot> {
        let unread = std::mem::take(&mut self.completed_unread);
        (unread && self.has_last).then(|| self.last.clone())
    }

    /// Frames folded into the current, not yet closed, window.
    pub fn partial_frames(&self) -> u32 {
        self.frames
    }
}

/// Appends `rows[index]` and, depth-first, every substage that ran. Writes
/// into fixed-capacity storage; the length guard is a capacity backstop.
/// Rows in a parent cycle are never roots, so they are dropped, not looped.
fn push_subtree(rows: &[Accumulator], index: usize, out: &mut Vec<WindowRow>) {
    if out.len() >= rows.len() {
        return;
    }
    let row = &rows[index];
    out.push(WindowRow {
        label: row.label,
        parent: row.parent,
        kind: row.kind,
        aggregate: row.aggregate,
        frames: row.frames,
        average: row.sum as f64 / f64::from(row.frames),
        max: row.max,
    });
    for (child, candidate) in rows.iter().enumerate() {
        if candidate.frames > 0 && candidate.parent == Some(row.label) {
            push_subtree(rows, child, out);
        }
    }
}

impl Default for StageWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(samples: &[(&'static str, Option<&'static str>, u64)]) -> FrameRecord {
        let mut record = FrameRecord::new();
        for &(label, parent, nanos) in samples {
            record.push_time(label, parent, nanos);
        }
        record
    }

    #[test]
    fn frame_120_closes_window_one_and_frame_121_opens_window_two() {
        let mut window = StageWindow::new();
        for index in 1..=WINDOW_FRAMES {
            let closed = window.fold(&frame(&[("stage", None, 1_000)]));
            assert_eq!(closed, index == WINDOW_FRAMES);
        }
        assert_eq!(window.last_window().unwrap().frames, WINDOW_FRAMES);
        assert_eq!(window.partial_frames(), 0);

        window.fold(&frame(&[("stage", None, 9_000)]));
        assert_eq!(window.partial_frames(), 1);
        let first = window.last_window().unwrap();
        assert_eq!(
            first.row("stage").unwrap().max,
            1_000,
            "nothing carries across"
        );
    }

    #[test]
    fn stage_averages_over_only_the_frames_it_ran_in() {
        let mut window = StageWindow::new();
        for index in 0..WINDOW_FRAMES {
            let mut record = frame(&[("visibility", None, 400)]);
            if index % 2 == 0 {
                record.push_time("walk", Some("visibility"), 100 + index as u64);
            } else {
                record.push_marker("fallback", Some("visibility"));
            }
            window.fold(&record);
        }
        let snapshot = window.last_window().unwrap();
        let walk = snapshot.row("walk").unwrap();
        assert_eq!(walk.frames, 60);
        let expected = (0..WINDOW_FRAMES)
            .step_by(2)
            .map(|index| 100.0 + f64::from(index))
            .sum::<f64>()
            / 60.0;
        assert!((walk.average - expected).abs() < 1e-9);
        assert!(walk.average <= snapshot.row("visibility").unwrap().average);
        assert_eq!(snapshot.row("fallback").unwrap().frames, 60);
    }

    #[test]
    fn stage_absent_from_every_frame_has_no_row() {
        let mut window = StageWindow::new();
        window.fold(&frame(&[("walk", None, 5)]));
        window.discard_partial();
        for _ in 0..WINDOW_FRAMES {
            window.fold(&frame(&[("other", None, 1)]));
        }
        let snapshot = window.last_window().unwrap();
        assert!(snapshot.row("walk").is_none());
        assert!(!snapshot.fields_string().contains("walk"));
    }

    #[test]
    fn discard_partial_keeps_the_last_window_and_clear_drops_it() {
        let mut window = StageWindow::new();
        for _ in 0..WINDOW_FRAMES {
            window.fold(&frame(&[("stage", None, 1)]));
        }
        for _ in 0..30 {
            window.fold(&frame(&[("stage", None, 50)]));
        }
        window.discard_partial();
        assert_eq!(window.partial_frames(), 0);
        assert!(window.last_window().is_some());
        for _ in 0..WINDOW_FRAMES {
            window.fold(&frame(&[("stage", None, 7)]));
        }
        assert_eq!(window.last_window().unwrap().row("stage").unwrap().max, 7);

        window.clear();
        assert!(window.last_window().is_none());
        assert!(window.take_completed_window().is_none());
    }

    #[test]
    fn completed_window_is_taken_once_and_last_window_stays_readable() {
        let mut window = StageWindow::new();
        for _ in 0..WINDOW_FRAMES {
            window.fold(&frame(&[("stage", None, 1)]));
        }
        assert!(window.take_completed_window().is_some());
        assert!(window.take_completed_window().is_none());
        assert!(window.last_window().is_some());
        assert_eq!(window.last_window(), window.last_window());
    }

    #[test]
    fn two_windows_in_one_process_never_share_samples() {
        let mut a = StageWindow::new();
        let mut b = StageWindow::new();
        for _ in 0..WINDOW_FRAMES {
            a.fold(&frame(&[("only_a", None, 3)]));
            b.fold(&frame(&[("only_b", None, 4)]));
        }
        assert!(a.last_window().unwrap().row("only_b").is_none());
        assert!(b.last_window().unwrap().row("only_a").is_none());
    }

    #[test]
    fn fields_mark_partial_rows_and_report_markers_as_counts() {
        let mut window = StageWindow::new();
        for index in 0..WINDOW_FRAMES {
            let mut record = frame(&[("total", None, 2_000_000)]);
            if index < 3 {
                record.push_marker("step_limit", None);
                record.push_count("considered", None, 20_000);
            }
            window.fold(&record);
        }
        let fields = window.last_window().unwrap().fields_string();
        assert!(fields.contains("total=2.000/2.000ms"), "{fields}");
        assert!(fields.contains("step_limit=3"), "{fields}");
        assert!(!fields.contains("step_limit=3("), "{fields}");
        assert!(
            fields.contains("considered=20000.0/20000(3/120)"),
            "{fields}"
        );
    }

    #[test]
    fn rows_come_out_in_depth_first_order_whatever_order_they_were_first_seen() {
        let mut window = StageWindow::new();
        for index in 0..WINDOW_FRAMES {
            let mut record = FrameRecord::new();
            record.push_aggregate_time("total", 10);
            record.push_time("wait", None, 2);
            record.push_time("stage_a", None, 3);
            record.push_time("stage_b", None, 4);
            record.push_time("wait_acquire", Some("wait"), 1);
            record.push_time("b_child", Some("stage_b"), 2);
            if index > 0 {
                // First seen after its parent's siblings, and after window start.
                record.push_time("a_child", Some("stage_a"), 1);
            }
            window.fold(&record);
        }
        let labels: Vec<_> = window
            .last_window()
            .unwrap()
            .rows
            .iter()
            .map(|row| row.label)
            .collect();
        assert_eq!(
            labels,
            [
                "total",
                "wait",
                "wait_acquire",
                "stage_a",
                "a_child",
                "stage_b",
                "b_child"
            ]
        );
        let total = window.last_window().unwrap().row("total").unwrap();
        assert!(total.aggregate);
    }
}
