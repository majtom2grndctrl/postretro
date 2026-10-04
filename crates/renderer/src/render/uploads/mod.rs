// Deferred renderer uploads and the single submission boundary.
// See: context/lib/rendering_pipeline.md §1, §4

use std::cell::{Cell, RefCell};
use std::panic::Location;

#[cfg(test)]
mod allocation_tests;
#[cfg(test)]
mod drift_tests;
pub(crate) mod staged_uploads;
mod staging_pool;
pub(crate) use staged_uploads::StagedUploads;
pub(crate) use staging_pool::StagingPool;

// Measured campaign writer batches fit a 1 MiB size class.
// Retain four such buffers for two frames in flight; small uniform-only batches
// use 64 KiB. Larger one-off batches may allocate but cannot grow this free cap.
const FRAME_STAGING_MIN_BYTES: u64 = 64 * 1024;
const FRAME_STAGING_MAX_BYTES: u64 = 4 * 1024 * 1024;
const FRAME_STAGING_MAX_BUFFERS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UploadError {
    Alignment,
    Bounds,
    TexturePayload,
}
impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "renderer upload rejected: {self:?}")
    }
}
impl std::error::Error for UploadError {}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UploadCounts {
    pub writes: u64,
    pub direct_writes: u64,
    pub submits: u64,
    pub batches: u64,
    pub copies: u64,
    pub bytes: u64,
    pub batches_first: u64,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct WriterCount {
    pub site: &'static Location<'static>,
    pub writes: u64,
}

#[cfg(test)]
impl WriterCount {
    /// Whether this writer sits at `line` of a file ending in `suffix`.
    pub(crate) fn is_at(&self, suffix: &str, line: u32) -> bool {
        self.site.line() == line && site_file_ends_with(self.site.file(), suffix)
    }
}

/// Suffix match for a `Location::file` path against a `/`-separated suffix.
/// `Location::file` uses the host separator, so Windows paths carry `\`.
#[cfg(test)]
pub(crate) fn site_file_ends_with(file: &str, suffix: &str) -> bool {
    file.replace('\\', "/").ends_with(suffix)
}

pub(crate) struct UploadQueue {
    device: wgpu::Device,
    raw: wgpu::Queue,
    enabled: Cell<bool>,
    batch: RefCell<StagedUploads>,
    pool: RefCell<StagingPool>,
    counts: Cell<UploadCounts>,
    writers: RefCell<Vec<WriterCount>>,
    diagnostics: bool,
    window_frames: Cell<u32>,
    window_start: Cell<UploadCounts>,
    window_max_live: Cell<u64>,
    window_max_batch_bytes: Cell<u64>,
}

impl UploadQueue {
    pub(crate) fn new(device: &wgpu::Device, raw: wgpu::Queue, enabled: bool) -> Self {
        Self {
            device: device.clone(),
            raw,
            enabled: Cell::new(enabled),
            batch: RefCell::new(StagedUploads::default()),
            pool: RefCell::new(StagingPool::bounded(
                FRAME_STAGING_MIN_BYTES,
                FRAME_STAGING_MAX_BYTES,
                FRAME_STAGING_MAX_BUFFERS,
            )),
            counts: Cell::new(UploadCounts::default()),
            writers: RefCell::new(Vec::with_capacity(128)),
            diagnostics: cfg!(test) || std::env::var_os("POSTRETRO_UPLOAD_TIMING").is_some(),
            window_frames: Cell::new(0),
            window_start: Cell::new(UploadCounts::default()),
            window_max_live: Cell::new(0),
            window_max_batch_bytes: Cell::new(0),
        }
    }

    /// Only installation and third-party resource owners receive the raw queue.
    pub(crate) fn raw(&self) -> &wgpu::Queue {
        &self.raw
    }
    pub(crate) fn enable(&self) {
        self.assert_empty("enabling frame uploads");
        self.enabled.set(true);
    }

    #[track_caller]
    pub(crate) fn write_buffer(&self, target: &wgpu::Buffer, offset: u64, data: &[u8]) {
        if self.enabled.get() {
            self.batch
                .borrow_mut()
                .write_buffer(target, offset, data)
                .unwrap_or_else(|error| panic!("{error}"));
            self.count_write(Location::caller(), false, data.is_empty());
        } else {
            self.direct_write_buffer(target, offset, data);
        }
    }

    #[track_caller]
    pub(crate) fn direct_write_buffer(&self, target: &wgpu::Buffer, offset: u64, data: &[u8]) {
        #[cfg(debug_assertions)]
        assert!(
            !self.batch.borrow().touches_buffer(target),
            "direct write follows a pending batched write to the same buffer"
        );
        self.raw.write_buffer(target, offset, data);
        self.count_write(Location::caller(), true, data.is_empty());
    }

    fn count_write(&self, site: &'static Location<'static>, direct: bool, empty: bool) {
        if empty {
            return;
        }
        let mut counts = self.counts.get();
        if direct {
            counts.direct_writes += 1;
        } else {
            counts.writes += 1;
        }
        self.counts.set(counts);
        if self.diagnostics {
            let mut writers = self.writers.borrow_mut();
            if let Some(writer) = writers.iter_mut().find(|writer| writer.site == site) {
                writer.writes += 1;
            } else {
                writers.push(WriterCount { site, writes: 1 });
            }
        }
    }

    /// Copies precede every command of the first submission after a write.
    pub(crate) fn submit(
        &self,
        commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> wgpu::SubmissionIndex {
        let mut batch = self.batch.borrow_mut();
        let mut pool = self.pool.borrow_mut();
        let mut counts = self.counts.get();
        counts.submits += 1;
        let recorded = if batch.is_empty() {
            None
        } else {
            counts.batches += 1;
            counts.batches_first += 1;
            counts.copies += batch.copy_count() as u64;
            counts.bytes += batch.byte_len() as u64;
            if self.diagnostics {
                self.window_max_batch_bytes.set(
                    self.window_max_batch_bytes
                        .get()
                        .max(batch.byte_len() as u64),
                );
            }
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Per-frame Upload Copies"),
                });
            let staging = batch
                .record_reusing(&mut pool, &self.device, &mut encoder)
                .expect("nonempty upload batch records a staging buffer");
            Some((encoder.finish(), staging))
        };
        self.counts.set(counts);
        let (first, staging) = match recorded {
            Some((command, buffer)) => (Some(command), Some(buffer)),
            None => (None, None),
        };
        // This is the only raw renderer submission, including boot and readbacks.
        let index = self.raw.submit(first.into_iter().chain(commands));
        if let Some(staging) = staging {
            pool.recycle(staging);
        }
        index
    }

    pub(crate) fn submit_unbatched(
        &self,
        commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
        boundary: &str,
    ) -> wgpu::SubmissionIndex {
        self.assert_empty(boundary);
        self.submit(commands)
    }

    pub(crate) fn flush_skipped_frame(&self) {
        if !self.batch.borrow().is_empty() {
            self.submit(std::iter::empty());
        }
        self.assert_empty("skipped frame exit");
    }

    pub(crate) fn assert_empty(&self, boundary: &str) {
        debug_assert!(
            self.batch.borrow().is_empty(),
            "pending renderer uploads at {boundary}"
        );
    }

    /// Load/install writes remain on the direct queue timeline; never interleave
    /// them with pending frame writes.
    pub(crate) fn installation(&self) -> Installation<'_> {
        self.assert_empty("level install/unload");
        let previous = self.enabled.replace(false);
        Installation {
            queue: self,
            previous,
        }
    }

    pub(crate) fn complete_frame(&self) {
        self.assert_empty("completed frame exit");
        if !self.diagnostics {
            return;
        }
        let frames = self.window_frames.get() + 1;
        let live = self.pool_counts().2;
        self.window_max_live
            .set(self.window_max_live.get().max(live));
        self.window_frames.set(frames);
        if frames == 120 {
            let now = self.counts();
            let before = self.window_start.get();
            let batches = now.batches - before.batches;
            log::info!(
                "[Renderer uploads] frames=120 staged_writes_per_frame={:.2} copies_per_batch={:.2} bytes_per_frame={:.2} batches={} direct_writes={} live_staging_max={} created_staging={} batch_bytes_max={}",
                (now.writes - before.writes) as f64 / 120.0,
                (now.copies - before.copies) as f64 / batches.max(1) as f64,
                (now.bytes - before.bytes) as f64 / 120.0,
                batches,
                now.direct_writes - before.direct_writes,
                self.window_max_live.get(),
                self.pool_counts().1,
                self.window_max_batch_bytes.get()
            );
            for writer in self.writer_counts().iter() {
                log::debug!(
                    "[Renderer uploads] site={} staged_writes={}",
                    writer.site,
                    writer.writes
                );
            }
            self.window_start.set(now);
            self.window_frames.set(0);
            self.window_max_live.set(0);
            self.window_max_batch_bytes.set(0);
        }
    }

    pub(crate) fn counts(&self) -> UploadCounts {
        self.counts.get()
    }
    pub(crate) fn pool_counts(&self) -> (u64, u64, u64) {
        self.pool.borrow().counts()
    }
    pub(crate) fn writer_counts(&self) -> std::cell::Ref<'_, Vec<WriterCount>> {
        self.writers.borrow()
    }
}

pub(crate) struct Installation<'a> {
    queue: &'a UploadQueue,
    previous: bool,
}
impl Drop for Installation<'_> {
    fn drop(&mut self) {
        self.queue.enabled.set(self.previous);
    }
}

#[cfg(test)]
mod renderer_tests;
#[cfg(test)]
mod tests;
