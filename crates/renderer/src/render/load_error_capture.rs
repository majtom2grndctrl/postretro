// Uncaptured wgpu errors while a level load is in flight: held as that load's failure instead of wgpu's default panic.
// See: context/plans/in-progress/level-load-pipeline/index.md (The commit cannot fail on level data)

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::Renderer;

/// The device's uncaptured-error handler. Disarmed, it does what wgpu's
/// default handler does: logs and panics. Armed for one load (a token the
/// caller chooses), it records the first error for that load and returns,
/// so the caller can route it as the load's one failure on a later frame.
///
/// wgpu calls the handler on the thread that made the failing call, after
/// releasing its own lock. The upload thread wraps every job in error
/// scopes, so in practice an armed capture sees errors that first surface
/// on the main thread (a submit, say).
#[derive(Debug, Default)]
pub(crate) struct LoadErrorCapture {
    state: Mutex<CaptureState>,
}

#[derive(Debug, Default)]
struct CaptureState {
    /// The load errors are recorded for; `None` panics as wgpu does.
    armed: Option<u64>,
    /// The first error recorded while armed, with the token it was armed for.
    first: Option<(u64, String)>,
}

impl LoadErrorCapture {
    /// A capture installed as `device`'s uncaptured-error handler, disarmed.
    pub(crate) fn install(device: &wgpu::Device) -> Arc<Self> {
        let capture = Arc::new(Self::default());
        let handler = Arc::clone(&capture);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            handler.handle(error);
        }));
        capture
    }

    fn lock(&self) -> MutexGuard<'_, CaptureState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn handle(&self, error: wgpu::Error) {
        let message = error.to_string();
        if self.record(&message) {
            log::error!("[Renderer] wgpu error during a level load, failing that load: {message}");
            return;
        }
        // wgpu's own default handler, word for word: outside a load an
        // uncaptured error stays fatal.
        log::error!("Handling wgpu errors as fatal by default");
        panic!("wgpu error: {message}\n");
    }

    /// Record `message` for the armed load; `false` when disarmed. The lock
    /// is released before the caller panics.
    fn record(&self, message: &str) -> bool {
        let mut state = self.lock();
        let Some(token) = state.armed else {
            return false;
        };
        if state.first.is_none() {
            state.first = Some((token, message.to_string()));
        }
        true
    }

    fn arm(&self, token: u64) {
        let mut state = self.lock();
        state.armed = Some(token);
        state.first = None;
    }

    fn take(&self, token: u64) -> Option<String> {
        let mut state = self.lock();
        match state.first.take() {
            Some((recorded, message)) if recorded == token => Some(message),
            // Recorded for another load: that load is gone, and its errors
            // reach nothing.
            Some(_) | None => None,
        }
    }

    fn disarm(&self) {
        let mut state = self.lock();
        state.armed = None;
        state.first = None;
    }
}

impl Renderer {
    /// Hold every uncaptured wgpu error from now on as load `token`'s
    /// failure instead of panicking, until [`Self::disarm_load_error_capture`]
    /// or the next arm. Clears any error held for an earlier load.
    pub fn arm_load_error_capture(&self, token: u64) {
        self.load_errors.arm(token);
    }

    /// The first uncaptured error held for load `token`, once. An error held
    /// for any other token is dropped.
    pub fn take_load_error(&self, token: u64) -> Option<String> {
        self.load_errors.take(token)
    }

    /// Back to wgpu's default: an uncaptured error panics. Drops any error
    /// still held.
    pub fn disarm_load_error_capture(&self) {
        self.load_errors.disarm();
    }

    /// Make one real wgpu validation error on the calling thread, outside any
    /// error scope: a buffer that is both mappable for reading and a storage
    /// buffer. For tests of the uncaptured-error route.
    #[cfg(feature = "test-seams")]
    pub fn provoke_uncaptured_validation_error(&self) {
        use super::gpu_create::GpuCreate;
        let _invalid = self.device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("provoked uncaptured validation error"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_armed_capture_holds_the_first_error_for_its_load_only() {
        let capture = LoadErrorCapture::default();
        assert!(!capture.record("before"), "disarmed: wgpu's panic");
        capture.arm(7);
        assert!(capture.record("first"));
        assert!(capture.record("second"));
        assert_eq!(capture.take(8), None, "another load's token reads nothing");
        capture.arm(7);
        assert!(capture.record("again"));
        assert_eq!(capture.take(7).as_deref(), Some("again"));
        assert_eq!(capture.take(7), None, "taken once");
        capture.disarm();
        assert!(!capture.record("after"));
    }

    #[test]
    fn a_disarmed_capture_panics_as_wgpu_does() {
        let capture = LoadErrorCapture::default();
        let panic = std::panic::catch_unwind(|| {
            capture.handle(wgpu::Error::Validation {
                source: Box::new(std::io::Error::other("bad")),
                description: "bad buffer".to_string(),
            })
        })
        .expect_err("outside a load an uncaptured error is fatal");
        let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.starts_with("wgpu error: "), "{message}");
    }
}
