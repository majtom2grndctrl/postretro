// Performance tab CPU block: the latest closed CPU stage window as a tree.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::{StageKind, WindowSnapshot};

/// What the binary's CPU timer can show this frame.
#[derive(Debug, Clone, Copy)]
pub enum CpuTimingPanel<'a> {
    /// `POSTRETRO_CPU_TIMING` is not `1`.
    Off,
    /// Timing is on, but no window has closed since startup or the last reset.
    NotYetWindowed,
    Window(&'a WindowSnapshot),
}

pub(super) fn draw_cpu_timing(ui: &mut egui::Ui, panel: CpuTimingPanel<'_>) {
    egui::CollapsingHeader::new("CPU Timing")
        .default_open(true)
        .show(ui, |ui| match panel {
            CpuTimingPanel::Off => {
                ui.label("CPU timing off (set POSTRETRO_CPU_TIMING=1)");
            }
            CpuTimingPanel::NotYetWindowed => {
                ui.label("CPU timing: no window closed yet");
            }
            CpuTimingPanel::Window(window) => {
                ui.label(format!("{} frames, avg/max", window.frames));
                for row in cpu_timing_rows(window) {
                    ui.monospace(row);
                }
            }
        });
}

/// One line per stage that ran, indented by tree depth. A stage that did not
/// run in the window has no line, so absent never reads as zero. Stages that
/// ran in only part of the window carry `(ran/frames)`.
pub(super) fn cpu_timing_rows(window: &WindowSnapshot) -> Vec<String> {
    window
        .rows
        .iter()
        .map(|row| {
            let indent = "  ".repeat(window.depth(row));
            let value = match row.kind {
                StageKind::Time => format!("{:.3} / {:.3} ms", row.average_ms(), row.max_ms()),
                StageKind::Count => format!("{:.1} / {}", row.average, row.max),
                StageKind::Marker => format!("{} frames", row.frames),
            };
            let partial = if row.kind != StageKind::Marker && row.frames < window.frames {
                format!(" ({}/{})", row.frames, window.frames)
            } else {
                String::new()
            };
            format!("{indent}{}: {value}{partial}", row.label)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_stage_timing::{FrameRecord, StageWindow, WINDOW_FRAMES};

    #[test]
    fn rows_indent_substages_and_omit_stages_that_did_not_run() {
        let mut window = StageWindow::new();
        for index in 0..WINDOW_FRAMES {
            let mut record = FrameRecord::new();
            record.push_time("render", None, 4_000_000);
            record.push_time("render_record", Some("render"), 3_000_000);
            if index % 4 == 0 {
                record.push_marker("portal_fallback", None);
            }
            window.fold(&record);
        }
        let rows = cpu_timing_rows(window.last_window().unwrap());
        assert_eq!(
            rows,
            [
                "render: 4.000 / 4.000 ms",
                "  render_record: 3.000 / 3.000 ms",
                "portal_fallback: 30 frames",
            ]
        );
        assert!(rows.iter().all(|row| !row.contains("sim_tick")));
    }

    #[test]
    fn partial_rows_show_how_many_frames_they_ran_in() {
        let mut window = StageWindow::new();
        for index in 0..WINDOW_FRAMES {
            let mut record = FrameRecord::new();
            record.push_time("total", None, 1_000_000);
            if index < 10 {
                record.push_time("sim_tick", Some("total"), 500_000);
            }
            window.fold(&record);
        }
        let rows = cpu_timing_rows(window.last_window().unwrap());
        assert_eq!(rows[1], "  sim_tick: 0.500 / 0.500 ms (10/120)");
    }
}
