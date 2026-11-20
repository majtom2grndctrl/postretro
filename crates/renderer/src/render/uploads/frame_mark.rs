// Playback frame marks: a debug group naming the playback frame index around
// each frame's drain and frame encoders, so a GPU trace reduces per frame.
// See: context/lib/rendering_pipeline.md §12

use std::fmt::Write as _;

/// The current playback frame's debug-group label. Formatted into one reused
/// `String`, so a mark allocates nothing once the label has grown to its
/// longest index.
#[derive(Debug, Default)]
pub(crate) struct FrameMark {
    label: String,
    active: bool,
}

impl FrameMark {
    /// `Some(k)` labels every marked encoder `pr-frame <k>` until the next
    /// set; `None` leaves every encoder unmarked.
    pub(crate) fn set(&mut self, frame: Option<u64>) {
        self.label.clear();
        self.active = false;
        if let Some(frame) = frame {
            write!(self.label, "pr-frame {frame}").expect("string write");
            self.active = true;
        }
    }

    pub(crate) fn label(&self) -> Option<&str> {
        self.active.then_some(self.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_mark_labels_the_playback_frame_and_clears_to_unmarked() {
        let mut mark = FrameMark::default();
        assert_eq!(mark.label(), None, "a renderer starts unmarked");
        mark.set(Some(7));
        assert_eq!(mark.label(), Some("pr-frame 7"));
        mark.set(Some(1_234));
        assert_eq!(mark.label(), Some("pr-frame 1234"));
        mark.set(None);
        assert_eq!(mark.label(), None, "playback inactive marks nothing");
    }

    #[test]
    fn frame_mark_reuses_its_label_storage_across_frames() {
        let mut mark = FrameMark::default();
        mark.set(Some(u64::MAX));
        let storage = mark.label.as_ptr();
        let capacity = mark.label.capacity();
        for frame in 0..1_000 {
            mark.set(Some(frame));
        }
        assert_eq!(mark.label.as_ptr(), storage, "the label never reallocates");
        assert_eq!(mark.label.capacity(), capacity);
    }

    /// Marking brackets an encoder in a debug group; it never renames one,
    /// so per-encoder trace reducers group as they always have.
    // Reads source text, so a reflow of those descriptors would trip it.
    #[test]
    fn marked_encoders_keep_their_labels() {
        for (source, label) in [
            (
                include_str!("../../lighting/lightmap/stream/execute.rs"),
                "label: Some(\"Streamed Lightmap Drain\")",
            ),
            (
                include_str!("staged_uploads.rs"),
                "label: Some(\"Streamed SH Upload Copies\")",
            ),
            (
                include_str!("../renderer_render_frame.rs"),
                "label: Some(\"Frame Encoder\")",
            ),
        ] {
            assert!(source.contains(label), "{label}");
        }
    }

    /// Marked and unmarked encoders both finish and submit without a
    /// validation error: the group is pushed only while marked, and every
    /// push is popped before `finish`.
    #[test]
    fn upload_queue_frame_mark_brackets_an_encoder_only_while_marked() {
        let Some(ctx) = crate::render::gpu_test_harness::gpu_or_skip(
            "upload_queue_frame_mark_brackets_an_encoder_only_while_marked",
        ) else {
            return;
        };
        let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame mark test buffer"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for frame in [Some(3), None] {
            queue.set_frame_mark(frame);
            let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame mark test encoder"),
                });
            let marked = queue.open_frame_mark(&mut encoder, super::super::MarkSite::Frame);
            assert_eq!(marked, frame.is_some(), "pushed only while marked");
            encoder.clear_buffer(&buffer, 0, None);
            queue.close_frame_mark(&mut encoder, super::super::MarkSite::Frame, marked);
            queue.submit(std::iter::once(encoder.finish()));
            let error = pollster::block_on(scope.pop());
            assert!(error.is_none(), "frame {frame:?}: {error:?}");
        }
    }
}
