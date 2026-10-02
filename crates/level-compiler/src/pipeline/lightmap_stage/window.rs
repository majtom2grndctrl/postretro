//! Bounded sliding window over (layer, light) lightmap partitions, consumed in
//! item order. See: context/lib/build_pipeline.md §Progress reporting.
//!
//! Each admitted item either loads its partition or fans out one task per
//! chart; each chart task enters the governor once, and nothing else here holds
//! a permit. The worker that completes an item's last chart assembles the
//! partition and runs the source's `finish` (the cache put) outside any permit.
//! Finished partitions wait in a ready set; whichever worker completes the next
//! item in order takes the consumer lock and drains the ready set in order, then
//! admits new items. No worker waits on another: a worker that cannot take the
//! consumer lock leaves its partition for the current holder, which re-checks
//! the ready set after releasing the lock.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rayon::ScopeFifo;

use crate::governor::Governor;
use crate::lightmap_layer::{LayerTexel, LightmapLayer};

/// Lights whose partitions may be in flight or awaiting consumption at once.
/// Bounds resident partitions; see `lightmap_stage::predicted_peak`.
pub(crate) const LIGHTMAP_PARTITION_WINDOW: usize = 8;

/// Where an item's partition comes from. Every method runs on a Rayon worker.
pub(super) trait PartitionSource: Sync {
    /// Cache read for `item`, outside any permit. `Some` skips the chart bake.
    fn load(&self, item: usize) -> Option<LightmapLayer>;
    /// Charts `item` bakes, in the order their texels are concatenated.
    fn charts(&self, item: usize) -> &[usize];
    /// One governed (light, chart) work unit: enters the governor exactly once.
    fn bake_chart(&self, item: usize, chart: usize) -> Vec<LayerTexel>;
    /// Assemble the concatenated chart texels into a partition and write it to
    /// the cache, outside any permit.
    fn finish(&self, item: usize, texels: Vec<LayerTexel>) -> LightmapLayer;
}

/// Test instrumentation: holds and observers at the window's orderings.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct WindowProbe {
    resident: Arc<AtomicUsize>,
    max_resident: Arc<AtomicUsize>,
    consumed: Arc<Mutex<Vec<usize>>>,
    /// Called with the item index when its partition becomes resident.
    on_ready: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

#[cfg(test)]
impl WindowProbe {
    pub(crate) fn with_on_ready(on_ready: impl Fn(usize) + Send + Sync + 'static) -> Self {
        Self {
            on_ready: Some(Arc::new(on_ready)),
            ..Self::default()
        }
    }

    /// Most partitions held at once between assembly (or load) and consumption.
    pub(crate) fn max_resident(&self) -> usize {
        self.max_resident.load(Ordering::SeqCst)
    }

    /// Item indices in the order the consumer received them.
    pub(crate) fn consumed(&self) -> Vec<usize> {
        self.consumed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct InFlight {
    slots: Mutex<Vec<Option<Vec<LayerTexel>>>>,
    remaining: AtomicUsize,
}

struct Consumer<F> {
    consume: F,
    next_admit: usize,
}

struct Window<'w, S, F> {
    source: &'w S,
    governor: &'w Governor,
    item_count: usize,
    window: usize,
    ready: Mutex<BTreeMap<usize, LightmapLayer>>,
    next_consume: AtomicUsize,
    consumer: Mutex<Consumer<F>>,
    #[cfg(test)]
    probe: Option<WindowProbe>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Produce one partition per item and hand each to `consume` in item order,
/// with at most `window` items admitted but not yet consumed.
pub(super) fn consume_in_order<S, F>(
    source: &S,
    governor: &Governor,
    item_count: usize,
    window: usize,
    consume: F,
    #[cfg(test)] probe: Option<WindowProbe>,
) where
    S: PartitionSource,
    F: FnMut(usize, LightmapLayer) + Send,
{
    assert!(window > 0, "lightmap partition window must be at least one");
    if item_count == 0 {
        return;
    }
    let first_batch = window.min(item_count);
    let state = Window {
        source,
        governor,
        item_count,
        window,
        ready: Mutex::new(BTreeMap::new()),
        next_consume: AtomicUsize::new(0),
        consumer: Mutex::new(Consumer {
            consume,
            next_admit: first_batch,
        }),
        #[cfg(test)]
        probe,
    };
    rayon::scope_fifo(|scope| {
        for item in 0..first_batch {
            let state = &state;
            scope.spawn_fifo(move |scope| state.start(scope, item));
        }
    });
    debug_assert_eq!(state.next_consume.load(Ordering::SeqCst), item_count);
}

impl<'w, S, F> Window<'w, S, F>
where
    S: PartitionSource,
    F: FnMut(usize, LightmapLayer) + Send,
{
    fn start<'s>(&'s self, scope: &ScopeFifo<'s>, item: usize) {
        if let Some(partition) = self.source.load(item) {
            // A hit holds no permit; it still honours pause.
            self.governor.checkpoint();
            self.ready(scope, item, partition);
            return;
        }
        let charts = self.source.charts(item);
        if charts.is_empty() {
            let partition = self.source.finish(item, Vec::new());
            self.ready(scope, item, partition);
            return;
        }
        let in_flight = Arc::new(InFlight {
            slots: Mutex::new(vec![None; charts.len()]),
            remaining: AtomicUsize::new(charts.len()),
        });
        for (ordinal, &chart) in charts.iter().enumerate() {
            let in_flight = Arc::clone(&in_flight);
            scope.spawn_fifo(move |scope| {
                let texels = self.source.bake_chart(item, chart);
                lock(&in_flight.slots)[ordinal] = Some(texels);
                if in_flight.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                    let texels: Vec<LayerTexel> = std::mem::take(&mut *lock(&in_flight.slots))
                        .into_iter()
                        .flat_map(|slot| slot.expect("every chart slot is filled"))
                        .collect();
                    self.mark_resident(item);
                    let partition = self.source.finish(item, texels);
                    self.ready_marked(scope, item, partition);
                }
            });
        }
    }

    fn ready<'s>(&'s self, scope: &ScopeFifo<'s>, item: usize, partition: LightmapLayer) {
        self.mark_resident(item);
        self.ready_marked(scope, item, partition);
    }

    fn ready_marked<'s>(&'s self, scope: &ScopeFifo<'s>, item: usize, partition: LightmapLayer) {
        lock(&self.ready).insert(item, partition);
        loop {
            let Ok(mut consumer) = self.consumer.try_lock() else {
                // The holder re-checks the ready set after it unlocks.
                return;
            };
            self.drain(scope, &mut consumer);
            drop(consumer);
            let next = self.next_consume.load(Ordering::SeqCst);
            if !lock(&self.ready).contains_key(&next) {
                return;
            }
        }
    }

    fn drain<'s>(&'s self, scope: &ScopeFifo<'s>, consumer: &mut Consumer<F>) {
        loop {
            let next = self.next_consume.load(Ordering::SeqCst);
            let Some(partition) = lock(&self.ready).remove(&next) else {
                return;
            };
            self.governor.checkpoint();
            (consumer.consume)(next, partition);
            self.release_resident(next);
            self.next_consume.store(next + 1, Ordering::SeqCst);
            while consumer.next_admit < self.item_count
                && consumer.next_admit < next + 1 + self.window
            {
                let admitted = consumer.next_admit;
                consumer.next_admit += 1;
                scope.spawn_fifo(move |scope| self.start(scope, admitted));
            }
        }
    }

    #[cfg_attr(not(test), allow(unused_variables))]
    fn mark_resident(&self, item: usize) {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            let now = probe.resident.fetch_add(1, Ordering::SeqCst) + 1;
            probe.max_resident.fetch_max(now, Ordering::SeqCst);
            if let Some(on_ready) = &probe.on_ready {
                on_ready(item);
            }
        }
    }

    #[cfg_attr(not(test), allow(unused_variables))]
    fn release_resident(&self, item: usize) {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.resident.fetch_sub(1, Ordering::SeqCst);
            lock(&probe.consumed).push(item);
        }
    }
}
