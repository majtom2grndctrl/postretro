// Microbenchmark for the per-frame fog cell-mask OR loop.
// See: context/plans/in-progress/perf-portal-fog-culling/index.md Task 4
//
// The fog pass calls `union_active_mask` once per frame on the visible-cell
// list to decide which fog volumes the raymarch shader should iterate. The
// plan target is < 10 µs on a synthetic 200-leaf input; this bench documents
// that target via criterion's statistical estimate. A separate, more
// generous (50 µs) ceiling lives as a `#[test]` in
// `postretro-level-format::fog_cell_masks` so algorithmic regressions trip
// `cargo test` without false-positives on loaded CI machines.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use postretro_level_format::fog_cell_masks::union_active_mask;

/// Build a synthetic input mirroring a small-to-medium map's per-frame
/// visibility set: 200 visible leaves with arbitrary fog-volume bitmasks.
/// Mask values cycle through every `MAX_FOG_VOLUMES = 16` bit so the union
/// loop sees realistic bit-set churn rather than a degenerate constant.
fn synthetic_inputs() -> (Vec<u32>, Vec<u32>) {
    // 1024 leaves total — well above the visible subset so out-of-range
    // safety isn't tripped, but small enough to fit in L1.
    let leaf_count = 1024usize;
    let masks: Vec<u32> = (0..leaf_count).map(|i| 1u32 << ((i as u32) % 16)).collect();
    let visible: Vec<u32> = (0..200u32).map(|i| (i * 5) % leaf_count as u32).collect();
    (visible, masks)
}

fn bench_union_active_mask(c: &mut Criterion) {
    let (visible, masks) = synthetic_inputs();

    c.bench_function("fog_cull/union_active_mask/200_leaves", |b| {
        b.iter(|| {
            // black_box defeats const-folding so the compiler can't hoist the
            // OR result out of the loop body.
            let m = union_active_mask(black_box(&visible), black_box(&masks));
            black_box(m);
        });
    });
}

criterion_group!(benches, bench_union_active_mask);
criterion_main!(benches);
