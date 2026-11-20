// GPU proofs of the drain flags camera-route playback records per frame: a
// cold drain made its staging pool create a buffer, and a growth drain grew
// a pool. Read through the counters the renderer hands playback.
// See: context/lib/rendering_pipeline.md §12, §4 (Lightmap cell-block residency)

use super::*;
use crate::render::PlaybackDrainCounters;

fn counters(stream: &Stream) -> PlaybackDrainCounters {
    PlaybackDrainCounters::of_parts(
        Some((
            stream.state.staging_buffers_created(),
            stream.state.counters(),
        )),
        None,
    )
}

/// Lets the drain's staging buffer return to its pool.
fn settle(stream: &Stream) {
    stream
        .ctx
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll stream test device");
}

#[test]
fn a_drain_creating_staging_is_cold_and_one_reusing_it_or_a_frame_upload_is_not() {
    let Some(mut stream) = Stream::new(
        "a_drain_creating_staging_is_cold_and_one_reusing_it_or_a_frame_upload_is_not",
        &[(128, 128), (128, 128)],
        1,
    ) else {
        return;
    };
    let before = counters(&stream);
    let outcome = stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    assert_eq!(outcome.installed, vec![0]);
    let first = counters(&stream);
    assert!(
        first.lightmap_cold_since(before),
        "the first drain creates staging"
    );
    assert!(!first.sh_cold_since(before), "no SH pool in this level");
    settle(&stream);

    let outcome = stream.drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]));
    assert_eq!(outcome.installed, vec![1]);
    let second = counters(&stream);
    assert!(
        !second.lightmap_cold_since(first),
        "a drain of the same size reuses the pooled buffer"
    );
    settle(&stream);

    // A per-frame upload draws on the upload queue's own pool: it may create
    // a buffer there, never in a drain's pool.
    let target = stream.ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame upload target"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let frame_pool_before = stream.queue.pool_counts().1;
    stream.queue.write_buffer(&target, 0, &[7u8; 256]);
    stream.queue.submit(std::iter::empty());
    assert!(
        stream.queue.pool_counts().1 > frame_pool_before,
        "the per-frame upload created a buffer in its own pool"
    );
    assert_eq!(counters(&stream), second, "no drain counter moved");
}

#[test]
fn a_drain_that_grows_the_lightmap_pool_flags_growth_and_one_that_fits_does_not() {
    let Some(mut stream) = Stream::new(
        "a_drain_that_grows_the_lightmap_pool_flags_growth_and_one_that_fits_does_not",
        &[(WIDE, 1100), (WIDE, 1200)],
        1,
    ) else {
        return;
    };
    let before = counters(&stream);
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let fitted = counters(&stream);
    assert!(
        !fitted.lightmap_pool_grew_since(before),
        "block 0 fits the first layer"
    );
    assert!(!fitted.mask_pool_grew_since(before));

    let outcome = stream.drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]));
    assert!(outcome.pool.lightmap.grew);
    let grown = counters(&stream);
    assert!(
        grown.lightmap_pool_grew_since(fitted),
        "block 1 needs a second layer"
    );
    assert!(
        !grown.mask_pool_grew_since(fitted),
        "the mask pool holds every plane from the start"
    );
}
