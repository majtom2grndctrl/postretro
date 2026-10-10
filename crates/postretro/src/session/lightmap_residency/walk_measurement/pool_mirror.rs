//! The renderer's lightmap drain without the GPU, for the residency walks.
//! See: context/lib/experimental_spikes.md

// The real `LightmapPoolModel` plans every batch as the renderer's lightmap
// stream drain does, and the outcome is built the same way. Texture work is
// not executed; its size is counted from the plan.

use std::time::Instant;

use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload, LightmapHeader,
};
use postretro_level_format::shadowmask_atlas::group_plane_len;
use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome, LightmapStreamManifest};
use postretro_render_cpu::lightmap_pool::LightmapPoolModel;

/// Drains a grown-out generation stays alive before its submitted work is
/// reported done. The renderer polls an `on_submitted_work_done` flag at
/// drain start; two frames in flight is the stand-in.
const RETIRE_AFTER_DRAINS: u32 = 2;

#[derive(Debug, Default, Clone)]
pub(super) struct PoolStats {
    pub(super) drains: u64,
    pub(super) submissions: u64,
    pub(super) uploads: u64,
    pub(super) install_bytes: u64,
    pub(super) repacks: u64,
    pub(super) repack_copies: u64,
    pub(super) growths: u64,
    pub(super) deferred: u64,
    pub(super) evictions: u64,
    pub(super) refusals: u64,
    pub(super) failed: u64,
    pub(super) first_layers: u32,
    pub(super) peak_layers: u32,
    pub(super) peak_occupied_layers: u32,
    /// Active plus retiring pool bytes, spare layers included, at their peak.
    pub(super) peak_pool_bytes: u64,
    /// Largest active-plus-retiring bytes a growth held at once.
    pub(super) growth_transient_peak_bytes: u64,
    /// CPU time of the model's planning per submitted drain, nanoseconds.
    pub(super) plan_nanos: Vec<u64>,
    /// Payload bytes each submitted drain uploaded.
    pub(super) drain_install_bytes: Vec<u64>,
    /// The settled-entry preload's one drain: pairs uploaded and their bytes.
    pub(super) preload_uploads: u64,
    pub(super) preload_install_bytes: u64,
}

pub(super) struct PoolMirror {
    model: LightmapPoolModel,
    header: LightmapHeader,
    with_shadowmask: bool,
    content_tag: [u8; 32],
    /// Bytes of one array layer across the irradiance, direction and (when
    /// kept) shadowmask planes.
    layer_bytes: u64,
    /// Array layers of the retiring generation and drains until release.
    retiring: Option<(u32, u32)>,
    pub(super) stats: PoolStats,
}

impl PoolMirror {
    pub(super) fn new(manifest: &LightmapStreamManifest, cap_layers: u32) -> Self {
        let index = manifest.lightmap_index();
        let header = index.header;
        let extents = index
            .records
            .iter()
            .map(|r| (u32::from(r.width), u32::from(r.height)))
            .collect();
        let model = LightmapPoolModel::new(
            extents,
            header.block_alignment(),
            LIGHTMAP_POOL_LAYER_EDGE,
            cap_layers,
        )
        .expect("the loader checked every block fits a pool layer");
        let with_shadowmask = manifest.shadowmask_index().is_some();
        let edge = LIGHTMAP_POOL_LAYER_EDGE;
        let layer_bytes = header.irradiance_len(edge, edge).expect("layer size")
            + header.direction_len(edge, edge).expect("layer size")
            + if with_shadowmask {
                2 * group_plane_len(edge, edge).expect("layer size")
            } else {
                0
            };
        let first_layers = model.layers();
        let mut mirror = Self {
            model,
            header,
            with_shadowmask,
            content_tag: manifest.content_tag(),
            layer_bytes,
            retiring: None,
            stats: PoolStats {
                first_layers,
                peak_layers: first_layers,
                ..PoolStats::default()
            },
        };
        mirror.stats.peak_pool_bytes = mirror.active_bytes();
        mirror
    }

    pub(super) fn model(&self) -> &LightmapPoolModel {
        &self.model
    }

    pub(super) fn layer_bytes(&self) -> u64 {
        self.layer_bytes
    }

    /// The active generation's bytes, spare layer included.
    pub(super) fn active_bytes(&self) -> u64 {
        u64::from(self.model.layers() + 1) * self.layer_bytes
    }

    /// Moves the settled-entry preload's drain out of the per-drain figures, so they
    /// describe in-play drains only. Pool shape and peaks carry over.
    pub(super) fn begin_play(&mut self) {
        let s = &mut self.stats;
        s.preload_uploads = s.uploads;
        s.preload_install_bytes = s.install_bytes;
        s.drains = 0;
        s.submissions = 0;
        s.uploads = 0;
        s.install_bytes = 0;
        s.plan_nanos.clear();
        s.drain_install_bytes.clear();
    }

    pub(super) fn drain(&mut self, batch: LightmapDrainBatch) -> LightmapDrainOutcome {
        batch
            .validate_contract(self.model.block_count(), self.content_tag)
            .expect("the controller's batch honours the drain contract");
        if let Some((layers, left)) = self.retiring {
            if left == 0 {
                self.model.release_retirement();
                self.retiring = None;
            } else {
                self.retiring = Some((layers, left - 1));
            }
        }

        let started = Instant::now();
        self.model.plan_batch(&batch);
        let mut failed = Vec::new();
        for upload in &self.model.plan().uploads {
            let fits = batch
                .ready
                .iter()
                .find(|p| p.block == upload.block)
                .is_some_and(|p| self.payload_fits((upload.width, upload.height), &p.payload));
            if !fits {
                failed.push(upload.block);
            }
        }
        for &block in &failed {
            self.model
                .fail_install(block)
                .expect("a failed block is one of this drain's planned uploads");
        }
        let plan_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);

        let plan = self.model.plan();
        let submitted = plan.growth.is_some()
            || !plan.copies.is_empty()
            || !plan.uploads.is_empty()
            || !plan.table_writes.is_empty();
        let drain_install_bytes: u64 = plan
            .uploads
            .iter()
            .filter_map(|u| batch.ready.iter().find(|p| p.block == u.block))
            .map(|p| payload_bytes(&p.payload))
            .sum();
        let s = &mut self.stats;
        s.drains += 1;
        if submitted {
            s.submissions += 1;
            s.plan_nanos.push(plan_nanos);
            s.drain_install_bytes.push(drain_install_bytes);
        }
        s.uploads += plan.uploads.len() as u64;
        s.install_bytes += drain_install_bytes;
        s.deferred += plan.deferred.len() as u64;
        s.evictions += plan.evicted.len() as u64;
        s.refusals += plan.refused.len() as u64;
        s.failed += plan.failed.len() as u64;
        if plan.report.repacked {
            s.repacks += 1;
            s.repack_copies += plan.copies.len() as u64;
        }
        let growth = plan.growth;
        s.peak_occupied_layers = s.peak_occupied_layers.max(self.model.occupied_layers());
        s.peak_layers = s.peak_layers.max(self.model.layers());
        if let Some(growth) = growth {
            s.growths += 1;
            self.retiring = Some((growth.from_layers + 1, RETIRE_AFTER_DRAINS));
        }
        let retiring_bytes = self
            .retiring
            .map_or(0, |(layers, _)| u64::from(layers) * self.layer_bytes);
        let held = self.active_bytes() + retiring_bytes;
        let s = &mut self.stats;
        s.peak_pool_bytes = s.peak_pool_bytes.max(held);
        if growth.is_some() {
            s.growth_transient_peak_bytes = s.growth_transient_peak_bytes.max(held);
        }
        self.outcome(batch)
    }

    fn payload_fits(&self, (width, height): (u32, u32), payload: &LightmapBlockPayload) -> bool {
        let header = &self.header;
        if header.irradiance_len(width, height) != Some(payload.irradiance.len() as u64)
            || header.direction_len(width, height) != Some(payload.direction.len() as u64)
        {
            return false;
        }
        if !self.with_shadowmask {
            return true;
        }
        let expected = group_plane_len(width, height);
        matches!(&payload.shadowmask, Some([a, b])
            if expected == Some(a.len() as u64) && expected == Some(b.len() as u64))
    }

    /// As `LightmapStreamState::outcome`.
    fn outcome(&self, batch: LightmapDrainBatch) -> LightmapDrainOutcome {
        let plan = self.model.plan();
        let deferred = if plan.deferred.is_empty() {
            Vec::new()
        } else {
            batch
                .ready
                .into_iter()
                .filter(|prepared| plan.deferred.binary_search(&prepared.block).is_ok())
                .collect()
        };
        LightmapDrainOutcome {
            installed: plan.installed.clone(),
            refused: plan.refused.clone(),
            failed: plan.failed.clone(),
            deferred,
            evicted: plan.evicted.iter().map(|eviction| eviction.block).collect(),
            pool: plan.report,
        }
    }
}

fn payload_bytes(payload: &LightmapBlockPayload) -> u64 {
    (payload.irradiance.len()
        + payload.direction.len()
        + payload
            .shadowmask
            .as_ref()
            .map_or(0, |[a, b]| a.len() + b.len())) as u64
}
