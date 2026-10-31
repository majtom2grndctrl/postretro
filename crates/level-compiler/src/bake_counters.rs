// Test-only bake cost counters: per-thread tallies summed over a dedicated pool.
// See: context/lib/testing_guide.md §3 (Resource bounds)

use std::cell::Cell;
use std::ops::Add;

/// What one bake spent, by unit of work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BakeCounts {
    /// Visibility segments traced (`segment_clear` in the SH and lightmap
    /// tracers): every shadow ray, whatever traced it.
    pub(crate) shadow_rays: u64,
    /// Probe gather rays (`sh_bake::gather_hit`).
    pub(crate) gather_rays: u64,
    /// Gather hits that read the surface radiance cache's patches.
    pub(crate) patch_lookups: u64,
    /// Patch-direct light evaluations: one per (patch, light) a layer shades.
    pub(crate) patch_light_evaluations: u64,
    /// Per-light layers shaded (a miss, or no cache).
    pub(crate) layers_shaded: u64,
    /// Per-light layers served from the build cache.
    pub(crate) layers_cached: u64,
    /// Layer cache reads and writes attempted.
    pub(crate) layer_cache_io: u64,
    /// Layer cache reads and writes attempted on a thread holding a governor
    /// permit: must stay zero.
    pub(crate) layer_cache_io_under_permit: u64,
}

impl Add for BakeCounts {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            shadow_rays: self.shadow_rays + other.shadow_rays,
            gather_rays: self.gather_rays + other.gather_rays,
            patch_lookups: self.patch_lookups + other.patch_lookups,
            patch_light_evaluations: self.patch_light_evaluations + other.patch_light_evaluations,
            layers_shaded: self.layers_shaded + other.layers_shaded,
            layers_cached: self.layers_cached + other.layers_cached,
            layer_cache_io: self.layer_cache_io + other.layer_cache_io,
            layer_cache_io_under_permit: self.layer_cache_io_under_permit
                + other.layer_cache_io_under_permit,
        }
    }
}

thread_local! {
    static COUNTS: Cell<BakeCounts> = const {
        Cell::new(BakeCounts {
            shadow_rays: 0,
            gather_rays: 0,
            patch_lookups: 0,
            patch_light_evaluations: 0,
            layers_shaded: 0,
            layers_cached: 0,
            layer_cache_io: 0,
            layer_cache_io_under_permit: 0,
        })
    };
}

/// Add to this thread's tally. Every thread counts; only [`counted`] reads.
pub(crate) fn bump(update: impl FnOnce(&mut BakeCounts)) {
    COUNTS.with(|counts| {
        let mut current = counts.get();
        update(&mut current);
        counts.set(current);
    });
}

fn take() -> BakeCounts {
    COUNTS.with(Cell::take)
}

/// Run `work` on a fresh pool of `threads` workers and return what it spent,
/// summed over the pool's threads. The pool runs no other test's work, so
/// the sum is exactly this bake's, and integer sums do not depend on which
/// worker ran what.
pub(crate) fn counted<R: Send>(threads: usize, work: impl FnOnce() -> R + Send) -> (R, BakeCounts) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("counting pool");
    pool.broadcast(|_| take());
    let result = pool.install(work);
    let counts = pool
        .broadcast(|_| take())
        .into_iter()
        .fold(BakeCounts::default(), Add::add);
    (result, counts)
}
