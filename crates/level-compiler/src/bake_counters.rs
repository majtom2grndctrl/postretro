// Test-only bake cost counters and gather-direction records, per thread over a dedicated pool.
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
    /// Later-bounce gather rays from patches (`sh_bake::bounce_hit`).
    pub(crate) bounce_rays: u64,
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
    /// Gathered-volume (id-34 entry) cache reads and writes attempted.
    pub(crate) volume_cache_io: u64,
    /// Gathered-volume cache reads and writes attempted on a thread holding a
    /// governor permit: must stay zero.
    pub(crate) volume_cache_io_under_permit: u64,
}

impl Add for BakeCounts {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            shadow_rays: self.shadow_rays + other.shadow_rays,
            gather_rays: self.gather_rays + other.gather_rays,
            bounce_rays: self.bounce_rays + other.bounce_rays,
            patch_lookups: self.patch_lookups + other.patch_lookups,
            patch_light_evaluations: self.patch_light_evaluations + other.patch_light_evaluations,
            layers_shaded: self.layers_shaded + other.layers_shaded,
            layers_cached: self.layers_cached + other.layers_cached,
            layer_cache_io: self.layer_cache_io + other.layer_cache_io,
            layer_cache_io_under_permit: self.layer_cache_io_under_permit
                + other.layer_cache_io_under_permit,
            volume_cache_io: self.volume_cache_io + other.volume_cache_io,
            volume_cache_io_under_permit: self.volume_cache_io_under_permit
                + other.volume_cache_io_under_permit,
        }
    }
}

thread_local! {
    static COUNTS: Cell<BakeCounts> = const {
        Cell::new(BakeCounts {
            shadow_rays: 0,
            gather_rays: 0,
            bounce_rays: 0,
            patch_lookups: 0,
            patch_light_evaluations: 0,
            layers_shaded: 0,
            layers_cached: 0,
            layer_cache_io: 0,
            layer_cache_io_under_permit: 0,
            volume_cache_io: 0,
            volume_cache_io_under_permit: 0,
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

/// Which gather traced a recorded direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Gather {
    /// The SH probe gather; the index is the probe's.
    Probe,
    /// A later-bounce patch gather; the index is the patch's seed id.
    Patch,
}

/// One traced gather ray: the gather, its probe or patch, the ray's index in
/// the direction set, and the world direction's bits.
pub(crate) type TracedDirection = (Gather, u64, u32, [u32; 3]);

thread_local! {
    static DIRECTIONS: std::cell::RefCell<Option<Vec<TracedDirection>>> =
        const { std::cell::RefCell::new(None) };
}

/// Record one gather ray on this thread, when [`directions_traced`] runs it.
pub(crate) fn record_direction(gather: Gather, index: u64, ray: u32, dir: glam::Vec3) {
    DIRECTIONS.with(|directions| {
        if let Some(directions) = directions.borrow_mut().as_mut() {
            directions.push((gather, index, ray, dir.to_array().map(f32::to_bits)));
        }
    });
}

/// Run `work` on a fresh pool of `threads` workers and return every gather
/// direction it traced, sorted, with repeats (a later bounce re-traces its
/// patch's set) kept.
pub(crate) fn directions_traced<R: Send>(
    threads: usize,
    work: impl FnOnce() -> R + Send,
) -> (R, Vec<TracedDirection>) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("recording pool");
    pool.broadcast(|_| DIRECTIONS.with(|directions| *directions.borrow_mut() = Some(Vec::new())));
    let result = pool.install(work);
    let mut traced: Vec<TracedDirection> = pool
        .broadcast(|_| DIRECTIONS.with(|directions| directions.borrow_mut().take()))
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    traced.sort_unstable();
    (result, traced)
}
