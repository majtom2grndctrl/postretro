// Shared fixtures of the pool policy tests: a model driven on the GPU mirror.

use postretro_level_loader::{LightmapBlockClass, LightmapTarget};

use super::gpu_mirror::GpuMirror;
use super::*;

pub(super) const EDGE: u32 = 64;
pub(super) const ALIGN: u32 = 4;

pub(super) fn target(block: u32, class: LightmapBlockClass, lead: u32) -> LightmapTarget {
    LightmapTarget { block, class, lead }
}

pub(super) fn mandatory(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Mandatory, 0)
}

pub(super) fn visible(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Visible, 0)
}

pub(super) fn band(block: u32, lead: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Band, lead)
}

/// A model plus the GPU mirror every plan executes on.
pub(super) struct Harness {
    pub model: LightmapPoolModel,
    pub gpu: GpuMirror,
    pub cap: u32,
}

impl Harness {
    pub(super) fn new(extents: &[(u32, u32)], cap: u32) -> Self {
        Self::with_layer_limit(extents, cap, u32::MAX)
    }

    pub(super) fn with_layer_limit(extents: &[(u32, u32)], cap: u32, max_layers: u32) -> Self {
        let extents = extents.to_vec();
        let model =
            LightmapPoolModel::with_layer_limit(extents, ALIGN, EDGE, cap, max_layers).unwrap();
        let gpu = GpuMirror::new(&model);
        Self { model, gpu, cap }
    }

    /// Plan without executing.
    pub(super) fn plan(
        &mut self,
        set: &[LightmapTarget],
        remove: &[u32],
        ready: &[u32],
    ) -> &DrainPlan {
        self.model.plan_drain(DrainRequest {
            pool_cap_layers: self.cap,
            target_reset: None,
            target_set: set,
            target_remove: remove,
            ready,
        })
    }

    pub(super) fn execute(&mut self) {
        self.gpu.execute(self.model.plan());
        self.gpu.assert_matches(&self.model);
    }

    pub(super) fn drain(
        &mut self,
        set: &[LightmapTarget],
        remove: &[u32],
        ready: &[u32],
    ) -> &DrainPlan {
        self.plan(set, remove, ready);
        self.execute();
        self.model.plan()
    }

    pub(super) fn evicted(&self) -> Vec<(u32, EvictionReason)> {
        self.model
            .plan()
            .evicted
            .iter()
            .map(|e| (e.block, e.reason))
            .collect()
    }
}

pub(super) fn assert_disjoint_within_layers(model: &LightmapPoolModel) {
    let rects: Vec<(u32, Slot)> = model
        .resident_blocks()
        .iter()
        .map(|&block| (block, model.slots[block as usize].unwrap()))
        .collect();
    for (i, &(a, sa)) in rects.iter().enumerate() {
        assert!(
            sa.layer < model.layers(),
            "block {a} past the active layers"
        );
        assert!(sa.x + sa.width <= EDGE && sa.y + sa.height <= EDGE);
        assert_eq!((sa.x % ALIGN, sa.y % ALIGN), (0, 0), "block {a} unaligned");
        for &(b, sb) in &rects[i + 1..] {
            let apart = sa.layer != sb.layer
                || sa.x + sa.width <= sb.x
                || sb.x + sb.width <= sa.x
                || sa.y + sa.height <= sb.y
                || sb.y + sb.height <= sa.y;
            assert!(apart, "blocks {a} and {b} overlap: {sa:?} {sb:?}");
        }
    }
}
