// The playback timestamp ring: slot-group bookkeeping and decode on the CPU,
// then real timestamps through marked encoders on an adapter that has them.
// See: context/lib/rendering_pipeline.md §12

use std::time::Duration;

use super::*;
use crate::render::gpu_test_harness::timestamp_gpu_or_skip;
use crate::render::uploads::UploadQueue;

fn ticks(pairs: [(u64, u64); 3]) -> [u64; 6] {
    // Slot order: lightmap, SH, whole frame.
    [
        pairs[0].0, pairs[0].1, pairs[1].0, pairs[1].1, pairs[2].0, pairs[2].1,
    ]
}

#[test]
fn playback_ring_gives_each_marked_frame_a_group_and_reuses_it_once_returned() {
    let mut ring = Ring::new();
    for frame in 0..RING_FRAMES as u64 {
        ring.begin_frame(frame);
        assert!(ring.open(MarkSite::Frame).is_some());
        let (group, _) = ring.close_frame().expect("a marked frame resolves");
        assert_eq!(group, frame as usize, "groups are taken in turn");
    }
    // Every group awaits its readback: the next marked frame goes untimed.
    ring.begin_frame(8);
    assert!(
        ring.open(MarkSite::Frame).is_none(),
        "no group, no query work"
    );
    assert!(ring.close_frame().is_none());
    assert_eq!(ring.untimed_frames, 1);

    // A returned readback frees its group for the next frame.
    ring.states[3] = GroupState::Idle;
    ring.begin_frame(9);
    assert_eq!(ring.recording.map(|(group, _)| group), Some(3));
    assert_eq!(ring.frames[3], 9);
}

#[test]
fn playback_ring_does_no_query_work_without_a_marked_frame() {
    let mut ring = Ring::new();
    for site in [
        MarkSite::LightmapDrain,
        MarkSite::ShUploads,
        MarkSite::Frame,
    ] {
        assert!(ring.open(site).is_none());
        assert!(ring.close_drain(site).is_none());
    }
    assert!(ring.close_frame().is_none());
    // A mark cleared before the frame encoder (no surface) resolves nothing
    // and gives its group back.
    ring.begin_frame(0);
    ring.open(MarkSite::LightmapDrain);
    ring.end_frame();
    assert!(ring.close_frame().is_none());
    assert!(ring.states.iter().all(|state| *state == GroupState::Idle));
}

#[test]
fn playback_ring_begins_the_whole_frame_on_the_first_marked_encoder_and_each_drain_once() {
    let mut ring = Ring::new();
    ring.begin_frame(4);
    let (_, lightmap) = ring.open(MarkSite::LightmapDrain).unwrap();
    assert_eq!(
        lightmap,
        [Some(Span::WholeFrame), Some(Span::LightmapDrain)]
    );
    let (_, sh) = ring.open(MarkSite::ShUploads).unwrap();
    assert_eq!(sh, [None, Some(Span::ShUploads)]);
    let (_, sh_again) = ring.open(MarkSite::ShUploads).unwrap();
    assert_eq!(
        sh_again,
        [None, None],
        "a second SH encoder extends the span"
    );
    let (_, frame) = ring.open(MarkSite::Frame).unwrap();
    assert_eq!(frame, [None, None]);
    let (group, unwritten) = ring.close_frame().unwrap();
    assert_eq!(unwritten, [None, None], "both drains wrote their slots");
    assert_eq!(ring.ran[group], [true, true]);

    // A frame with no drain begins the whole frame on its frame encoder and
    // writes zero-length pairs for both drains before resolving.
    ring.begin_frame(5);
    let (_, frame) = ring.open(MarkSite::Frame).unwrap();
    assert_eq!(frame, [Some(Span::WholeFrame), None]);
    let (group, unwritten) = ring.close_frame().unwrap();
    assert_eq!(
        unwritten,
        [Some(Span::LightmapDrain), Some(Span::ShUploads)]
    );
    assert_eq!(ring.ran[group], [false, false]);
}

#[test]
fn playback_ring_decodes_spans_and_marks_absent_and_malformed_apart() {
    let period = 1.0;
    let sample = decode_group(
        12,
        &ticks([(100, 400), (500, 900), (50, 2_000)]),
        [true, true],
        period,
    );
    assert_eq!(sample.frame, 12);
    assert_eq!(sample.lightmap_drain, PlaybackGpuSpan::Nanos(300));
    assert_eq!(sample.sh_upload, PlaybackGpuSpan::Nanos(400));
    assert_eq!(sample.whole_frame, PlaybackGpuSpan::Nanos(1_950));

    let absent = decode_group(1, &ticks([(9, 9), (9, 9), (1, 2)]), [false, false], period);
    assert_eq!(absent.lightmap_drain, PlaybackGpuSpan::Absent, "never zero");
    assert_eq!(absent.sh_upload, PlaybackGpuSpan::Absent);

    let over_a_second = decode_group(
        2,
        &ticks([(1, 1_000_000_002), (0, 5), (10, 5)]),
        [true, true],
        period,
    );
    assert_eq!(over_a_second.lightmap_drain, PlaybackGpuSpan::Malformed);
    assert_eq!(
        over_a_second.sh_upload,
        PlaybackGpuSpan::Malformed,
        "zero endpoint"
    );
    assert_eq!(
        over_a_second.whole_frame,
        PlaybackGpuSpan::Malformed,
        "end before start"
    );

    // The period converts ticks to nanoseconds.
    let scaled = decode_group(3, &ticks([(1, 1), (1, 1), (10, 20)]), [false, false], 2.5);
    assert_eq!(scaled.whole_frame, PlaybackGpuSpan::Nanos(25));
}

/// Encoder work long enough to read as a positive span.
fn clear(encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer) {
    encoder.clear_buffer(buffer, 0, None);
}

fn encoder(device: &wgpu::Device, label: &str) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
}

#[test]
fn playback_ring_times_marked_drain_and_frame_encoders_on_the_gpu() {
    let Some(ctx) =
        timestamp_gpu_or_skip("playback_ring_times_marked_drain_and_frame_encoders_on_the_gpu")
    else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    queue.enable_playback_timing();
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("playback ring test buffer"),
        size: 16 * 1024 * 1024,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    for frame in 0..4u64 {
        queue.set_frame_mark(Some(frame));
        // Frame 2 runs no lightmap drain.
        if frame != 2 {
            let mut lightmap = encoder(&ctx.device, "Streamed Lightmap Drain");
            let marked = queue.open_frame_mark(&mut lightmap, MarkSite::LightmapDrain);
            clear(&mut lightmap, &buffer);
            queue.close_frame_mark(&mut lightmap, MarkSite::LightmapDrain, marked);
            queue.submit(std::iter::once(lightmap.finish()));
        }
        let mut sh = encoder(&ctx.device, "Streamed SH Upload Copies");
        let marked = queue.open_frame_mark(&mut sh, MarkSite::ShUploads);
        clear(&mut sh, &buffer);
        queue.close_frame_mark(&mut sh, MarkSite::ShUploads, marked);
        queue.submit(std::iter::once(sh.finish()));
        let mut frame_encoder = encoder(&ctx.device, "Frame Encoder");
        let marked = queue.open_frame_mark(&mut frame_encoder, MarkSite::Frame);
        clear(&mut frame_encoder, &buffer);
        queue.close_frame_mark(&mut frame_encoder, MarkSite::Frame, marked);
        queue.submit(std::iter::once(frame_encoder.finish()));
        queue.playback_post_submit();
    }
    // An unmarked frame adds no query work.
    queue.set_frame_mark(None);
    let writes = queue.playback_timing().as_ref().unwrap().timestamp_writes();
    let mut unmarked = encoder(&ctx.device, "Frame Encoder");
    let marked = queue.open_frame_mark(&mut unmarked, MarkSite::Frame);
    assert!(!marked, "no debug group without a mark");
    clear(&mut unmarked, &buffer);
    queue.close_frame_mark(&mut unmarked, MarkSite::Frame, marked);
    queue.submit(std::iter::once(unmarked.finish()));
    queue.playback_post_submit();
    assert_eq!(
        queue.playback_timing().as_ref().unwrap().timestamp_writes(),
        writes,
        "an unmarked frame writes no timestamp"
    );

    let mut samples = Vec::new();
    let outstanding = queue.playback_timing().as_mut().unwrap().finish(
        &ctx.device,
        Duration::from_secs(5),
        &mut |sample| samples.push(sample),
    );
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
    assert_eq!(outstanding, 0, "every readback returned");
    samples.sort_by_key(|sample| sample.frame);
    assert_eq!(
        samples
            .iter()
            .map(|sample| sample.frame)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3],
        "one sample per marked frame, none for the unmarked one"
    );
    let positive_under_a_second = |span: PlaybackGpuSpan, what: &str| match span {
        PlaybackGpuSpan::Nanos(ns) => {
            assert!(ns > 0 && ns < 1_000_000_000, "{what}: {ns} ns");
        }
        other => panic!("{what}: {other:?}"),
    };
    for sample in &samples {
        let frame = sample.frame;
        positive_under_a_second(sample.whole_frame, &format!("frame {frame} whole"));
        positive_under_a_second(sample.sh_upload, &format!("frame {frame} SH"));
        if frame == 2 {
            assert_eq!(sample.lightmap_drain, PlaybackGpuSpan::Absent);
        } else {
            positive_under_a_second(sample.lightmap_drain, &format!("frame {frame} lightmap"));
        }
        if let (PlaybackGpuSpan::Nanos(whole), PlaybackGpuSpan::Nanos(sh)) =
            (sample.whole_frame, sample.sh_upload)
        {
            assert!(
                whole >= sh,
                "frame {frame}: the whole frame spans its SH copies"
            );
        }
    }
}
