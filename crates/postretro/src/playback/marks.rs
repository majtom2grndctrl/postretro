// What a playback row is marked with beside its timing: mid-run changes that
// persist to the run's end, hidden-window gaps, and pawn life-state changes.
// Pure: no App, window or GPU.
// See: context/lib/rendering_pipeline.md §12

use serde::ser::SerializeSeq;

/// A mid-run change that makes later rows unlike earlier ones (pin P10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    /// The vsync diagnostic chord switched the present mode.
    Vsync,
    /// A resize, scale-factor or render-resolution change rebuilt the extents.
    Extents,
    /// The window mode changed, or a pending change was kept or reverted.
    WindowMode,
    /// The drain-cap slider moved (Task 7's dev-tools slider marks it).
    DrainCap,
    /// A script hot reload committed.
    ScriptReload,
}

impl ChangeKind {
    const ALL: [Self; 5] = [
        Self::Vsync,
        Self::Extents,
        Self::WindowMode,
        Self::DrainCap,
        Self::ScriptReload,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Vsync => "vsync",
            Self::Extents => "extents",
            Self::WindowMode => "window_mode",
            Self::DrainCap => "drain_cap",
            Self::ScriptReload => "script_reload",
        }
    }
}

/// The changes a row carries; serialized as their keys.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChangeSet(u8);

impl ChangeSet {
    #[cfg(test)]
    pub(crate) const EMPTY: Self = Self(0);

    pub(crate) fn insert(&mut self, kind: ChangeKind) {
        self.0 |= kind.bit();
    }

    pub(crate) fn contains(self, kind: ChangeKind) -> bool {
        self.0 & kind.bit() != 0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl serde::Serialize for ChangeSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kinds = ChangeKind::ALL
            .into_iter()
            .filter(|kind| self.contains(*kind));
        let mut seq = serializer.serialize_seq(None)?;
        for kind in kinds {
            seq.serialize_element(kind.key())?;
        }
        seq.end()
    }
}

/// Mid-run change and gap marks. A change persists: every row from the
/// first one it touches to the run's end carries it. A gap marks only the
/// rows whose interval spans it: the first row after the window was hidden
/// or a frame presented nothing, and every row written while hidden.
#[derive(Debug, Default)]
pub(crate) struct Marks {
    changes: ChangeSet,
    occluded: bool,
    gap_pending: bool,
}

impl Marks {
    pub(crate) fn note_change(&mut self, kind: ChangeKind) {
        self.changes.insert(kind);
    }

    /// The window was hidden (`true`) or shown again.
    pub(crate) fn note_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        self.gap_pending |= occluded;
    }

    /// The marks the next row carries: every change so far, and whether its
    /// interval spans a gap.
    pub(crate) fn take_row_marks(&mut self) -> (ChangeSet, bool) {
        let gap = self.gap_pending || self.occluded;
        self.gap_pending = false;
        (self.changes, gap)
    }

    /// After a planned frame's row: a frame that presented nothing leaves a
    /// gap the next row's interval spans.
    pub(crate) fn after_frame(&mut self, presented: bool) {
        self.gap_pending |= !presented;
    }
}

/// The local pawn's life-state identity one frame saw: a change in any part
/// is a death, respawn, teleport or table change the record logs (pin P11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LifeKey {
    pub(crate) pawn: u32,
    pub(crate) teleport_generation: u8,
    pub(crate) life_state: u8,
    /// Entries into a life state, a respawn into the same state included.
    pub(crate) life_state_entry: u32,
    pub(crate) table_revision: u8,
}

/// Compares each frame's local pawn with the previous frame's.
#[derive(Debug, Default)]
pub(crate) struct LifeWatch {
    /// `None` until the first frame sets the baseline; then the previous
    /// frame's pawn, `Some(None)` when it had none.
    last: Option<Option<LifeKey>>,
}

impl LifeWatch {
    /// This frame's pawn; returns the previous frame's when they differ.
    /// The first frame only sets the baseline.
    pub(crate) fn observe(&mut self, now: Option<LifeKey>) -> Option<Option<LifeKey>> {
        let previous = self.last.replace(now)?;
        (previous != now).then_some(previous)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_persist_changes_to_every_later_row() {
        let mut marks = Marks::default();
        assert_eq!(marks.take_row_marks(), (ChangeSet::default(), false));
        marks.note_change(ChangeKind::Vsync);
        for _ in 0..3 {
            let (changes, gap) = marks.take_row_marks();
            assert!(changes.contains(ChangeKind::Vsync) && !gap);
        }
        marks.note_change(ChangeKind::ScriptReload);
        let (changes, _) = marks.take_row_marks();
        assert!(changes.contains(ChangeKind::Vsync) && changes.contains(ChangeKind::ScriptReload));
        assert_eq!(
            serde_json::to_value(changes).unwrap(),
            serde_json::json!(["vsync", "script_reload"])
        );
    }

    #[test]
    fn marks_gap_only_the_rows_that_span_it() {
        let mut marks = Marks::default();
        // Hidden, then shown: only the first row after carries the gap.
        marks.note_occluded(true);
        marks.note_occluded(false);
        assert!(marks.take_row_marks().1);
        assert!(!marks.take_row_marks().1);
        // Rows written while hidden are marked; the first after is too.
        marks.note_occluded(true);
        assert!(marks.take_row_marks().1);
        assert!(marks.take_row_marks().1);
        marks.note_occluded(false);
        assert!(!marks.take_row_marks().1, "nothing pending once shown");
        // A frame that presented nothing: the next row spans its gap.
        let _ = marks.take_row_marks();
        marks.after_frame(false);
        assert!(marks.take_row_marks().1);
        marks.after_frame(true);
        assert!(!marks.take_row_marks().1);
        assert_eq!(
            marks.take_row_marks().0,
            ChangeSet::default(),
            "a gap is no change"
        );
    }

    #[test]
    fn life_watch_reports_a_change_against_the_previous_frame() {
        let alive = LifeKey {
            pawn: 4,
            teleport_generation: 0,
            life_state: 0,
            life_state_entry: 1,
            table_revision: 1,
        };
        let dead = LifeKey {
            life_state: 1,
            life_state_entry: 2,
            ..alive
        };
        let respawned = LifeKey {
            life_state: 0,
            life_state_entry: 3,
            teleport_generation: 1,
            ..alive
        };
        let mut watch = LifeWatch::default();
        assert_eq!(
            watch.observe(Some(alive)),
            None,
            "the first frame is the baseline"
        );
        assert_eq!(watch.observe(Some(alive)), None);
        assert_eq!(watch.observe(Some(dead)), Some(Some(alive)));
        assert_eq!(watch.observe(Some(dead)), None);
        assert_eq!(watch.observe(Some(respawned)), Some(Some(dead)));
        assert_eq!(watch.observe(None), Some(Some(respawned)), "the pawn left");
        assert_eq!(watch.observe(None), None);
    }
}
