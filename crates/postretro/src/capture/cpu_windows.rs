// CPU stage windows over capture sample frames.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::{
    FrameRecord, StageFrame, StageSet, StageWindow, TimingGate, WindowSnapshot,
};

use crate::cpu_timing::{CpuStagesReport, capture_stages_report};

/// Folds each sample frame's renderer stages into 120-frame windows. Capture
/// runs no tick or walk per sample, so only renderer stages appear, and their
/// roots are top-level rows here rather than children of the windowed
/// `render` stage. Every complete window is kept once; the trailing partial
/// window is reported only as a frame count.
pub(super) struct CaptureCpuWindows {
    gate: TimingGate,
    window: Option<StageWindow>,
    record: FrameRecord,
    windows: Vec<WindowSnapshot>,
}

impl CaptureCpuWindows {
    pub(super) fn new(gate: TimingGate) -> Self {
        Self {
            gate,
            window: gate.is_enabled().then(StageWindow::new),
            record: FrameRecord::new(),
            windows: Vec::new(),
        }
    }

    /// Drops everything folded so far. Called once warmup ends, so no warmup
    /// frame reaches the report.
    pub(super) fn reset(&mut self) {
        if let Some(window) = self.window.as_mut() {
            window.clear();
        }
        self.windows.clear();
    }

    pub(super) fn fold_sample<S: StageSet>(&mut self, frame: &StageFrame<S>) {
        let Some(window) = self.window.as_mut() else {
            return;
        };
        self.record.clear();
        self.record.extend_from(frame, None);
        if window.fold(&self.record) {
            self.windows.extend(window.take_completed_window());
        }
    }

    pub(super) fn report(&self) -> CpuStagesReport {
        let partial = self.window.as_ref().map_or(0, StageWindow::partial_frames);
        capture_stages_report(self.gate, &self.windows, partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu_timing::CpuStagesReport;
    use postretro_renderer::cpu_stages::RenderStage;
    use postretro_stage_timing::WINDOW_FRAMES;

    fn sample(windows: &mut CaptureCpuWindows, forward_nanos: u64) {
        let frame = StageFrame::<RenderStage>::new(TimingGate::ON);
        frame.add_nanos(RenderStage::Record, forward_nanos + 10);
        frame.add_nanos(RenderStage::Forward, forward_nanos);
        windows.fold_sample(&frame);
    }

    fn json(report: &CpuStagesReport) -> serde_json::Value {
        serde_json::to_value(report).expect("serialize cpu_stages")
    }

    #[test]
    fn capture_reports_each_complete_post_warmup_window_once_and_a_partial_count() {
        // P-capture: warmup W = 130, samples S = 250.
        let mut windows = CaptureCpuWindows::new(TimingGate::ON);
        for _ in 0..130 {
            sample(&mut windows, 9_000_000);
        }
        windows.reset();
        for _ in 0..250 {
            sample(&mut windows, 1_000_000);
        }
        let report = json(&windows.report());
        assert_eq!(report["availability"], "available");
        assert!(report.get("reason").is_none());
        let reported = report["windows"].as_array().unwrap();
        assert_eq!(reported.len(), 2);
        for window in reported {
            assert_eq!(window["frames"], WINDOW_FRAMES);
            let forward = window["stages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stage| stage["label"] == "rec_forward")
                .unwrap();
            let max_ms = forward["max"].as_f64().unwrap();
            assert!(
                (max_ms - 1.0).abs() < 1e-9,
                "no warmup frame reached a window"
            );
            assert_eq!(forward["parent"], "render_record");
        }
        assert_eq!(report["partial_frames"], 10);
    }

    #[test]
    fn stages_that_never_ran_are_omitted_not_zero() {
        let mut windows = CaptureCpuWindows::new(TimingGate::ON);
        for _ in 0..WINDOW_FRAMES {
            sample(&mut windows, 1_000);
        }
        let report = json(&windows.report());
        let labels: Vec<_> = report["windows"][0]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| stage["label"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(labels, ["render_record", "rec_forward"]);
    }

    #[test]
    fn fewer_samples_than_a_window_report_not_yet_windowed() {
        let mut windows = CaptureCpuWindows::new(TimingGate::ON);
        for _ in 0..40 {
            sample(&mut windows, 1_000);
        }
        let report = json(&windows.report());
        assert_eq!(report["availability"], "not-yet-windowed");
        assert_eq!(report["reason"], "window-not-complete");
        assert!(report.get("windows").is_none());
        assert_eq!(report["partial_frames"], 40);
    }

    #[test]
    fn timing_off_reports_not_requested_with_a_reason() {
        let mut windows = CaptureCpuWindows::new(TimingGate::OFF);
        for _ in 0..WINDOW_FRAMES {
            sample(&mut windows, 1_000);
        }
        let report = json(&windows.report());
        assert_eq!(report["availability"], "not-requested");
        assert_eq!(report["reason"], "env-disabled");
        assert!(report.get("windows").is_none());
        assert!(report.get("partial_frames").is_none());
    }

    #[test]
    fn cpu_stages_section_round_trips() {
        let mut windows = CaptureCpuWindows::new(TimingGate::ON);
        for _ in 0..WINDOW_FRAMES + 3 {
            sample(&mut windows, 2_500);
        }
        let report = windows.report();
        let text = serde_json::to_string(&report).unwrap();
        let back: CpuStagesReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back, report);
    }
}
