// Playback frame marks: a debug group naming the playback frame index and the
// marked site around each frame's drain and frame encoders, so a GPU trace
// reduces per frame.
// See: context/lib/rendering_pipeline.md §12

use std::fmt::Write as _;

use super::MarkSite;

/// Each marked site and its name in the label, in label-slot order. The site
/// rides in the label because wgpu's Metal backend relabels the last command
/// buffer of every submit "(wgpu internal) Signal", so a trace reducer cannot
/// always tell a drain from its command buffer's label.
const SITES: [(MarkSite, &str); 3] = [
    (MarkSite::LightmapDrain, "lightmap"),
    (MarkSite::ShUploads, "sh"),
    (MarkSite::Frame, "frame"),
];

/// `site`'s index into [`SITES`] and the label slots.
fn slot(site: MarkSite) -> usize {
    match site {
        MarkSite::LightmapDrain => 0,
        MarkSite::ShUploads => 1,
        MarkSite::Frame => 2,
    }
}

/// The current playback frame's debug-group labels, one per marked site.
/// Each is formatted into its own reused `String`, so a mark allocates
/// nothing once the labels have grown to their longest index.
#[derive(Debug, Default)]
pub(crate) struct FrameMark {
    labels: [String; SITES.len()],
    active: bool,
}

impl FrameMark {
    /// `Some(k)` labels every marked encoder `pr-frame <k> <site>`, with
    /// `<site>` one of `lightmap`, `sh` or `frame`, until the next set;
    /// `None` leaves every encoder unmarked.
    pub(crate) fn set(&mut self, frame: Option<u64>) {
        for ((_, name), label) in SITES.iter().zip(self.labels.iter_mut()) {
            label.clear();
            if let Some(frame) = frame {
                write!(label, "pr-frame {frame} {name}").expect("string write");
            }
        }
        self.active = frame.is_some();
    }

    pub(crate) fn label(&self, site: MarkSite) -> Option<&str> {
        self.active.then_some(self.labels[slot(site)].as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_mark_labels_the_playback_frame_and_site_and_clears_to_unmarked() {
        let mut mark = FrameMark::default();
        for (site, _) in SITES {
            assert_eq!(mark.label(site), None, "a renderer starts unmarked");
        }
        mark.set(Some(7));
        assert_eq!(
            mark.label(MarkSite::LightmapDrain),
            Some("pr-frame 7 lightmap")
        );
        assert_eq!(mark.label(MarkSite::ShUploads), Some("pr-frame 7 sh"));
        assert_eq!(mark.label(MarkSite::Frame), Some("pr-frame 7 frame"));
        mark.set(Some(1_234));
        assert_eq!(mark.label(MarkSite::ShUploads), Some("pr-frame 1234 sh"));
        mark.set(None);
        for (site, _) in SITES {
            assert_eq!(mark.label(site), None, "playback inactive marks nothing");
        }
    }

    #[test]
    fn every_site_owns_its_label_slot() {
        for (index, (site, _)) in SITES.iter().enumerate() {
            assert_eq!(slot(*site), index, "{site:?}");
        }
    }

    #[test]
    fn frame_mark_reuses_its_label_storage_across_frames() {
        let mut mark = FrameMark::default();
        mark.set(Some(u64::MAX));
        let storage = mark.labels.each_ref().map(|label| label.as_ptr());
        let capacity = mark.labels.each_ref().map(String::capacity);
        for frame in 0..1_000 {
            mark.set(Some(frame));
        }
        assert_eq!(
            mark.labels.each_ref().map(|label| label.as_ptr()),
            storage,
            "no label reallocates"
        );
        assert_eq!(mark.labels.each_ref().map(String::capacity), capacity);
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
