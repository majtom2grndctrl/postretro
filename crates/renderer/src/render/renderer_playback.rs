// Camera-route playback's renderer seams beside the frame mark: the GPU
// timestamp ring and the drain counters a per-frame record reads.
// See: context/lib/rendering_pipeline.md §12

use std::time::Duration;

use super::*;

/// Cumulative drain-side counters of the installed level, read before and
/// after a frame: the difference says what that frame's drains did. Only the
/// two drains' own staging pools count; the per-frame uploads pool is apart,
/// so a buffer created for a per-frame upload is never a cold drain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackDrainCounters {
    pub lightmap_staging_created: u64,
    pub sh_staging_created: u64,
    pub lightmap_pool_growths: u64,
    pub mask_pool_growths: u64,
}

impl PlaybackDrainCounters {
    /// From the lightmap drain's staging-pool count and counters, and the SH
    /// residency uploads' staging-pool count; a resource that does not
    /// stream reads zero.
    pub(crate) fn of_parts(
        lightmap: Option<(u64, LightmapStreamCounters)>,
        sh_staging_created: Option<u64>,
    ) -> Self {
        let (lightmap_staging_created, growths) = lightmap
            .map_or((0, Default::default()), |(created, counters)| {
                (created, counters.growths)
            });
        Self {
            lightmap_staging_created,
            sh_staging_created: sh_staging_created.unwrap_or(0),
            lightmap_pool_growths: growths.lightmap,
            mask_pool_growths: growths.mask,
        }
    }

    /// The lightmap drain created a staging buffer after `before`.
    pub fn lightmap_cold_since(self, before: Self) -> bool {
        self.lightmap_staging_created > before.lightmap_staging_created
    }

    /// The SH residency uploads created a staging buffer after `before`.
    pub fn sh_cold_since(self, before: Self) -> bool {
        self.sh_staging_created > before.sh_staging_created
    }

    /// A lightmap drain grew the lightmap pool after `before`.
    pub fn lightmap_pool_grew_since(self, before: Self) -> bool {
        self.lightmap_pool_growths > before.lightmap_pool_growths
    }

    /// A lightmap drain grew the mask pool after `before`.
    pub fn mask_pool_grew_since(self, before: Self) -> bool {
        self.mask_pool_growths > before.mask_pool_growths
    }
}

impl Renderer {
    /// The installed level's drain counters; zero for a resource that does
    /// not stream.
    pub fn playback_drain_counters(&self) -> PlaybackDrainCounters {
        let level = self.full.as_ref().and_then(|full| full.level.as_ref());
        let sh_staging = level
            .and_then(|level| level.sh_streaming.as_ref())
            .and_then(|streaming| streaming.staging_buffers_created());
        let lightmap = level.and_then(|level| {
            let resources = &level.lightmap_resources;
            resources
                .stream_staging_buffers_created()
                .zip(resources.stream_counters())
        });
        PlaybackDrainCounters::of_parts(lightmap, sh_staging)
    }

    /// Enables the playback timestamp ring when per-pass GPU timing runs on
    /// this device (`POSTRETRO_GPU_TIMING=1` and both timestamp features),
    /// the same gate. Returns that gate's state; the ring exists only when
    /// it reads enabled. Every buffer the ring uses is created here.
    pub fn enable_playback_gpu_timing(&mut self) -> GpuTimingSupport {
        let support = self.gpu_timing_support();
        if support == GpuTimingSupport::Enabled {
            self.queue.enable_playback_timing();
        }
        support
    }

    /// Hands every playback GPU sample whose readback has landed to `sink`.
    /// Never blocks; readbacks advance after each frame's submit.
    pub fn take_playback_gpu_samples(&mut self, mut sink: impl FnMut(PlaybackGpuSample)) {
        if let Some(timing) = self.queue.playback_timing().as_mut() {
            timing.take_samples(&mut sink);
        }
    }

    /// The run's end: waits up to `timeout` for the readbacks still in
    /// flight, hands every sample to `sink`, and removes the ring. Returns
    /// the frames whose readback never returned, and the marked frames that
    /// found the ring full and went untimed.
    pub fn finish_playback_gpu_timing(
        &mut self,
        timeout: Duration,
        mut sink: impl FnMut(PlaybackGpuSample),
    ) -> (usize, u64) {
        let Some(mut timing) = self.queue.playback_timing().take() else {
            return (0, 0);
        };
        let outstanding = timing.finish(&self.device, timeout, &mut sink);
        (outstanding, timing.untimed_frames())
    }
}
