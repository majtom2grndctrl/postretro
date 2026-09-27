// A frame's gathered samples from every stage set, placed in one tree by label.
// See: context/lib/rendering_pipeline.md §12

use crate::{StageFrame, StageKind, StageSet};

/// Fixed capacity of one frame's gathered samples.
pub const MAX_FRAME_SAMPLES: usize = 128;

/// One stage's value for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub label: &'static str,
    /// Enclosing stage's label; `None` for a top-level frame stage.
    pub parent: Option<&'static str>,
    pub kind: StageKind,
    pub value: u64,
}

const EMPTY_SAMPLE: Sample = Sample {
    label: "",
    parent: None,
    kind: StageKind::Time,
    value: 0,
};

/// Every stage that ran in one frame, gathered by the binary from each
/// crate's [`StageFrame`]. Fixed storage: gathering never allocates.
#[derive(Clone)]
pub struct FrameRecord {
    samples: [Sample; MAX_FRAME_SAMPLES],
    len: usize,
    overflowed: bool,
}

impl FrameRecord {
    pub const fn new() -> Self {
        Self {
            samples: [EMPTY_SAMPLE; MAX_FRAME_SAMPLES],
            len: 0,
            overflowed: false,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.overflowed = false;
    }

    /// Adds one sample. A label already present this frame sums into it.
    pub fn push(&mut self, sample: Sample) {
        if let Some(existing) = self.samples[..self.len]
            .iter_mut()
            .find(|existing| existing.label == sample.label)
        {
            debug_assert_eq!(existing.parent, sample.parent, "{}", sample.label);
            existing.value = existing.value.saturating_add(sample.value);
            return;
        }
        if self.len == MAX_FRAME_SAMPLES {
            debug_assert!(false, "FrameRecord overflow at {}", sample.label);
            self.overflowed = true;
            return;
        }
        self.samples[self.len] = sample;
        self.len += 1;
    }

    pub fn push_time(&mut self, label: &'static str, parent: Option<&'static str>, nanos: u64) {
        self.push(Sample {
            label,
            parent,
            kind: StageKind::Time,
            value: nanos,
        });
    }

    pub fn push_count(&mut self, label: &'static str, parent: Option<&'static str>, count: u64) {
        self.push(Sample {
            label,
            parent,
            kind: StageKind::Count,
            value: count,
        });
    }

    pub fn push_marker(&mut self, label: &'static str, parent: Option<&'static str>) {
        self.push(Sample {
            label,
            parent,
            kind: StageKind::Marker,
            value: 1,
        });
    }

    /// Gathers every stage that ran in `frame`. The set's roots are placed
    /// under `anchor`; substages keep their in-set parent. No stage is named
    /// here, so a new stage in an existing set needs no caller change.
    pub fn extend_from<S: StageSet>(&mut self, frame: &StageFrame<S>, anchor: Option<&'static str>) {
        for &stage in S::ALL {
            if let Some(value) = frame.value(stage) {
                self.push(Sample {
                    label: stage.label(),
                    parent: stage.parent().map(StageSet::label).or(anchor),
                    kind: stage.kind(),
                    value,
                });
            }
        }
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples[..self.len]
    }

    pub fn value(&self, label: &str) -> Option<u64> {
        self.samples()
            .iter()
            .find(|sample| sample.label == label)
            .map(|sample| sample.value)
    }

    /// Replaces a present label's value. Used to move a blocking span out of
    /// its enclosing stage once it has been attributed elsewhere.
    pub fn set_value(&mut self, label: &str, value: u64) {
        if let Some(sample) = self.samples[..self.len]
            .iter_mut()
            .find(|sample| sample.label == label)
        {
            sample.value = value;
        }
    }

    /// Sum of the top-level time stages.
    pub fn top_level_time(&self) -> u64 {
        self.samples()
            .iter()
            .filter(|sample| sample.parent.is_none() && sample.kind == StageKind::Time)
            .map(|sample| sample.value)
            .sum()
    }

    /// Labels whose time exceeds their parent's this frame. Empty when the
    /// stage tree is consistent.
    pub fn substage_overruns(&self) -> impl Iterator<Item = &Sample> {
        self.samples().iter().filter(move |sample| {
            sample.kind == StageKind::Time
                && sample
                    .parent
                    .and_then(|parent| self.value(parent))
                    .is_some_and(|parent_value| sample.value > parent_value)
        })
    }

    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl Default for FrameRecord {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TimingGate;
    use crate::test_sets::Demo;

    #[test]
    fn set_roots_attach_to_the_anchor_and_substages_keep_their_parent() {
        let frame = StageFrame::<Demo>::new(TimingGate::ON);
        frame.add_nanos(Demo::Outer, 100);
        frame.add_nanos(Demo::Inner, 40);
        let mut record = FrameRecord::new();
        record.push_time("host", None, 500);
        record.extend_from(&frame, Some("host"));

        let outer = record.samples().iter().find(|s| s.label == "outer").unwrap();
        let inner = record.samples().iter().find(|s| s.label == "inner").unwrap();
        assert_eq!(outer.parent, Some("host"));
        assert_eq!(inner.parent, Some("outer"));
        assert_eq!(record.top_level_time(), 500);
        assert_eq!(record.substage_overruns().count(), 0);
    }

    #[test]
    fn stages_that_did_not_run_are_not_gathered() {
        let frame = StageFrame::<Demo>::new(TimingGate::ON);
        frame.add_nanos(Demo::Inner, 1);
        let mut record = FrameRecord::new();
        record.extend_from(&frame, None);
        assert_eq!(record.samples().len(), 1);
        assert_eq!(record.value("outer"), None);
    }

    #[test]
    fn substage_overrun_is_reported() {
        let mut record = FrameRecord::new();
        record.push_time("parent", None, 10);
        record.push_time("child", Some("parent"), 11);
        let overruns: Vec<_> = record.substage_overruns().map(|s| s.label).collect();
        assert_eq!(overruns, ["child"]);
    }
}
