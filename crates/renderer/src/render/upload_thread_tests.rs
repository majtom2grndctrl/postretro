// Upload-thread error capture: wgpu errors and panics become job results.
// See: context/lib/testing_guide.md §3 (GPU tests)

use super::*;
use crate::render::gpu_test_harness::gpu_or_skip;
use std::time::{Duration, Instant};

/// Poll `pending` until its job reports, failing the test after a bound.
pub(in crate::render) fn wait_for<T>(mut pending: PendingUpload<T>) -> Result<T, UploadError> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(result) = pending.try_take() {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "upload job did not report within 30 s"
        );
        std::thread::yield_now();
    }
}

#[test]
fn upload_thread_captures_validation_error_instead_of_panicking() {
    let Some(ctx) = gpu_or_skip("upload_thread_captures_validation_error_instead_of_panicking")
    else {
        return;
    };
    let thread = UploadThread::spawn(ctx.device.clone(), ctx.queue.clone()).unwrap();

    let overrun = thread.submit(|context: &UploadContext| {
        let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("upload thread overrun target"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // 32 bytes into a 16-byte buffer: a validation error, which wgpu's
        // default handler would turn into a panic.
        context.queue.write_buffer(&buffer, 0, &[0u8; 32]);
        Ok(())
    });
    match wait_for(overrun) {
        Err(UploadError::Wgpu(message)) => assert!(!message.is_empty()),
        other => panic!("a write past the buffer end must report a wgpu error, got {other:?}"),
    }

    let next = thread.submit(|context: &UploadContext| {
        let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("upload thread in-bounds target"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&buffer, 0, &[0u8; 16]);
        Ok(7u32)
    });
    assert_eq!(
        wait_for(next).expect("the thread keeps serving after a captured error"),
        7
    );
}

#[test]
fn upload_thread_reports_job_panic_as_error() {
    let Some(ctx) = gpu_or_skip("upload_thread_reports_job_panic_as_error") else {
        return;
    };
    let thread = UploadThread::spawn(ctx.device.clone(), ctx.queue.clone()).unwrap();

    let panicking = thread.submit(|_: &UploadContext| -> Result<(), UploadError> {
        panic!("injected upload job panic")
    });
    match wait_for(panicking) {
        Err(UploadError::Panicked(message)) => {
            assert!(message.contains("injected upload job panic"), "{message}")
        }
        other => panic!("a panicking job must report Panicked, got {other:?}"),
    }

    let next = thread.submit(|_: &UploadContext| Ok("still serving"));
    assert_eq!(wait_for(next).unwrap(), "still serving");
}
