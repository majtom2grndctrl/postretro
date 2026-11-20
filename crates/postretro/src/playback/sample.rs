// What a playback row measures beside its plan, read from the App around each
// world frame: the CPU split the timer committed, the drains' admission and
// flags, both late counts, the machine state, and the GPU ring's samples.
// See: context/lib/rendering_pipeline.md §12

use std::time::Duration;

use crate::App;
use crate::options::WindowMode;

use super::record::{
    AdapterReport, DisplayModeReport, RowDrain, RowFill, RunContext, WindowModeReport,
};

/// How long a run's end waits for GPU readbacks still in flight: a few
/// frames' worth, once, never per frame.
const GPU_READBACK_END_WAIT: Duration = Duration::from_secs(2);

impl App {
    /// The run arms: the record names the timing gates, and GPU timing
    /// gets playback's own query ring when the per-pass gate is on.
    pub(super) fn playback_arm_timing(&mut self) {
        let cpu = self.cpu_timer.gate().is_enabled();
        let gpu = self
            .renderer
            .as_mut()
            .map(|renderer| renderer.enable_playback_gpu_timing());
        if let Some(active) = self.playback.active.as_mut() {
            active.record.set_timing(
                cpu,
                gpu.unwrap_or(postretro_renderer::GpuTimingSupport::NotRequested),
            );
        }
    }

    /// A planned frame begins: its drains are measured from here.
    pub(super) fn playback_sample_begin(&mut self) {
        let before = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.playback_drain_counters())
            .unwrap_or_default();
        if let Some(active) = self.playback.active.as_mut() {
            active.drain_before = before;
            active.admission = Default::default();
        }
    }

    /// The machine state the run's first row records under.
    pub(super) fn playback_run_context(&self) -> Option<RunContext> {
        let renderer = self.renderer.as_ref()?;
        let identity = renderer.capture_measurement_adapter_identity();
        let extents = renderer.render_extents();
        let (mode, display) = self.window_modes.effective_mode();
        Some(RunContext {
            adapter: AdapterReport {
                name: identity.name.clone(),
                backend: identity.backend.clone(),
                device_type: identity.device_type.clone(),
            },
            surface_extent: [extents.surface.width, extents.surface.height],
            scene_extent: [extents.scene.width, extents.scene.height],
            render_divisor: extents.divisor,
            window_mode: WindowModeReport {
                mode: match mode {
                    WindowMode::Windowed => "windowed",
                    WindowMode::Borderless => "borderless",
                    WindowMode::Exclusive => "exclusive",
                },
                display: display.map(|display| DisplayModeReport {
                    width: display.width,
                    height: display.height,
                    refresh_millihertz: display.refresh_millihertz,
                    monitor: display.monitor.clone(),
                }),
            },
            present_mode: if renderer.vsync_enabled() {
                "vsync"
            } else {
                "no-vsync"
            },
        })
    }

    /// The measured columns of the frame ending now, after the CPU timer
    /// committed it; `None` when no planned frame is ending.
    pub(super) fn playback_row_fill(&self) -> Option<RowFill> {
        let active = self.playback.active.as_ref()?;
        active.run.current()?;
        let after = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.playback_drain_counters())
            .unwrap_or_default();
        let late = self.session.as_ref().map_or((None, None), |session| {
            (
                session.last_frame_lightmap_misses(),
                session.last_frame_sh_misses(),
            )
        });
        let drain = RowDrain::new(active.admission, active.drain_before, after, late);
        Some(RowFill::new(self.cpu_timer.committed_record(), drain))
    }

    /// Puts every landed GPU sample on its row. Never blocks.
    pub(super) fn playback_take_gpu_samples(&mut self) {
        let (Some(active), Some(renderer)) =
            (self.playback.active.as_mut(), self.renderer.as_mut())
        else {
            return;
        };
        if active.record.gpu_timing_enabled() {
            renderer.take_playback_gpu_samples(|sample| active.record.patch_gpu(sample));
        }
    }

    /// The run's end, before the record's write: waits briefly for the
    /// readbacks still in flight, places them, counts the rows that never
    /// got one, and removes the ring.
    pub(super) fn playback_finish_gpu(&mut self, active: &mut super::ActivePlayback) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let (outstanding, untimed) = renderer
            .finish_playback_gpu_timing(GPU_READBACK_END_WAIT, |sample| {
                active.record.patch_gpu(sample)
            });
        active.record.finish_gpu(outstanding, untimed);
    }
}
