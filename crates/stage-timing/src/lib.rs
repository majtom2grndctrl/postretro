// Per-stage CPU timing mechanism: stage sets, scope guard, frame record, window fold.
// See: context/lib/rendering_pipeline.md §12

//! Each engine crate declares its own [`StageSet`] and records into a
//! [`StageFrame`] that travels upward in that crate's existing outputs. The
//! binary gathers every crate's frame into one [`FrameRecord`], places set
//! roots under its own stages by label, and folds records into a
//! [`StageWindow`]. Nothing here names a stage, and there is no global state:
//! two windows in one process never see each other's samples.
//!
//! Allocation contract: scopes, frame recording and the per-frame fold never
//! allocate. A window allocates its fixed row storage once, at construction,
//! and window close reuses it. Only surfaces (log text, capture snapshots)
//! allocate, and at most once per window.

mod frame;
mod record;
mod window;

pub use frame::{StageFrame, StageScope};
pub use record::{FrameRecord, MAX_FRAME_SAMPLES, Sample};
pub use window::{StageWindow, WINDOW_FRAMES, WindowRow, WindowSnapshot};

/// Largest stage set a crate may declare. Bounds per-frame storage to a fixed
/// array so recording never allocates.
pub const MAX_STAGES_PER_SET: usize = 32;

/// Whether CPU stage timing is on. Read once at startup by the binary and
/// handed to each timed crate as a value, so tests switch it without touching
/// the process environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimingGate {
    enabled: bool,
}

impl TimingGate {
    pub const OFF: Self = Self { enabled: false };
    pub const ON: Self = Self { enabled: true };

    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub const fn is_enabled(self) -> bool {
        self.enabled
    }
}

/// What a stage's per-frame value means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    /// Nanoseconds of CPU time, summed over every entry in the frame.
    Time,
    /// A per-frame counter (portal considerations, ticks). Windowed like time.
    Count,
    /// A per-frame flag; the window reports only how many frames raised it.
    Marker,
}

/// A crate's closed, engine-owned stage vocabulary.
///
/// Labels must be unique across every set a binary folds together: windows
/// key rows by label, and parents are named by label. A stage whose
/// [`parent`](StageSet::parent) is `None` is a root of its set; the binary
/// places roots under one of its own stages when it gathers the frame.
pub trait StageSet: Copy + Eq + std::fmt::Debug + 'static {
    /// Every stage, in display order. At most [`MAX_STAGES_PER_SET`].
    const ALL: &'static [Self];

    /// Dense index into `ALL`.
    fn index(self) -> usize;

    fn label(self) -> &'static str;

    /// Enclosing stage within this set. A substage's time is inside its
    /// parent's, never added to it.
    fn parent(self) -> Option<Self> {
        None
    }

    fn kind(self) -> StageKind {
        StageKind::Time
    }
}

#[cfg(test)]
mod test_sets {
    use crate::{StageKind, StageSet};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Demo {
        Outer,
        Inner,
        Hits,
        Tripped,
    }

    impl StageSet for Demo {
        const ALL: &'static [Self] = &[Self::Outer, Self::Inner, Self::Hits, Self::Tripped];

        fn index(self) -> usize {
            self as usize
        }

        fn label(self) -> &'static str {
            match self {
                Self::Outer => "outer",
                Self::Inner => "inner",
                Self::Hits => "hits",
                Self::Tripped => "tripped",
            }
        }

        fn parent(self) -> Option<Self> {
            match self {
                Self::Inner => Some(Self::Outer),
                _ => None,
            }
        }

        fn kind(self) -> StageKind {
            match self {
                Self::Hits => StageKind::Count,
                Self::Tripped => StageKind::Marker,
                _ => StageKind::Time,
            }
        }
    }
}
