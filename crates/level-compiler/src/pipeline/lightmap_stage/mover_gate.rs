//! The mover light gate over the fused walk's partitions.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;

use crate::light_namespaces::StaticBakedEntry;
use crate::lightmap_layer::{LightmapLayer, SharedAtlas};
use crate::mover_light_gate::{
    CandidateMover, GateTally, MoverPoses, MoverTexels, gate_partition, light_candidates,
};

/// Gates each partition the compose pass bakes or loads, on the worker that
/// produced it, as the texel-step detector does. The serial consumer never
/// waits on it, and worker order never reaches the tally.
pub(super) struct MoverGateDetector<'a> {
    poses: &'a MoverPoses,
    shared: &'a SharedAtlas<'a>,
    layer_lights: &'a [&'a StaticBakedEntry<'a>],
    /// By position in `layer_lights`.
    candidates: Vec<Vec<CandidateMover>>,
    /// (item, light position, gated movers) per partition with a gated texel.
    found: Mutex<Vec<(usize, usize, Vec<MoverTexels>)>>,
    /// Gate time summed over workers.
    nanos: AtomicU64,
}

impl<'a> MoverGateDetector<'a> {
    /// `None` when no mover is analysed, so the walk pays nothing.
    pub(super) fn new(
        poses: Option<&'a MoverPoses>,
        shared: &'a SharedAtlas<'a>,
        layer_lights: &'a [&'a StaticBakedEntry<'a>],
    ) -> Option<Self> {
        let poses = poses.filter(|poses| !poses.analysed.is_empty())?;
        Some(Self {
            poses,
            shared,
            layer_lights,
            candidates: layer_lights
                .iter()
                .map(|entry| light_candidates(entry.light, poses))
                .collect(),
            found: Mutex::new(Vec::new()),
            nanos: AtomicU64::new(0),
        })
    }

    /// Whether light position `light` may meet any posed mover.
    pub(super) fn wants(&self, light: usize) -> bool {
        !self.candidates[light].is_empty()
    }

    /// Gate `partition`, item `item` of light position `light`, over
    /// `charts` of its layer with their padded texel bounds.
    pub(super) fn detect(
        &self,
        item: usize,
        light: usize,
        partition: &LightmapLayer,
        charts: impl Iterator<Item = (usize, Option<(Vec3, Vec3)>)>,
    ) {
        let started = Instant::now();
        let movers = gate_partition(
            self.poses,
            self.layer_lights[light].light,
            &self.candidates[light],
            self.shared,
            partition,
            charts,
        );
        if !movers.is_empty() {
            self.found
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((item, light, movers));
        }
        self.nanos
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// The rows, folded in item order, and the gate's time summed over
    /// workers.
    pub(super) fn finish(mut self) -> (GateTally, Duration) {
        let mut found = std::mem::take(
            self.found
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        found.sort_unstable_by_key(|(item, _, _)| *item);
        let mut tally = GateTally::default();
        for (_, light, movers) in &found {
            tally.add_partition(self.poses, self.layer_lights[*light].source_index, movers);
        }
        (
            tally,
            Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
        )
    }
}
