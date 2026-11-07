// Streamed lightmap and mask pools, GPU half: the placement model, each pool's
// live and retiring texture generations, the vertex block table, and each drain.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

#[cfg(test)]
mod compaction_timing_measurement;
mod counters;
mod execute;
#[cfg(test)]
mod install_timing_measurement;
#[cfg(test)]
mod tests;
mod textures;

use crate::render::gpu_create::GpuCreate;
use std::fmt;

use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapHeader};
use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome};
use postretro_render_cpu::lightmap_pool::{
    BLOCK_TABLE_ENTRY_BYTES, LightmapPoolModel, PerPool, PoolKind, payload_mismatch,
};

pub use counters::LightmapStreamCounters;
use textures::{PoolGeneration, PoolTextures};

use super::StreamingPoolPlan;
use super::pool::{PoolFormat, PoolPlane};
use crate::render::StagingPool;

/// A drain the renderer refused. Every variant leaves the pool, the table
/// and the model exactly as they were before the drain, except that a failed
/// execution keeps any retirement the drain released, since its GPU work was
/// already done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightmapResidencyDrainError {
    /// The installed level does not stream its lightmap.
    NotStreaming,
    /// The batch broke the drain contract: identity, block ids or ordering.
    InvalidBatch(String),
    /// The batch belongs to an older generation: a previous level's, or one
    /// this level has moved past.
    StaleGeneration { current: u64, received: u64 },
    /// A new generation must open with a target reset.
    GenerationResetRequired { current: u64, received: u64 },
    /// Growth needs more array layers (spare included) than the device has.
    /// The model plans within the device limit and defers a pair past it, so
    /// this means the model and the device disagree.
    GpuCapacity {
        required_layers: u32,
        max_layers: u32,
    },
    /// Staging a planned write failed.
    Upload(String),
}

impl fmt::Display for LightmapResidencyDrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStreaming => write!(f, "the installed level does not stream its lightmap"),
            Self::InvalidBatch(reason) => write!(f, "invalid lightmap drain batch: {reason}"),
            Self::StaleGeneration { current, received } => write!(
                f,
                "lightmap drain generation {received} is stale (renderer is at {current})"
            ),
            Self::GenerationResetRequired { current, received } => write!(
                f,
                "lightmap drain generation {received} (renderer at {current}) must open with a \
                 target reset"
            ),
            Self::GpuCapacity {
                required_layers,
                max_layers,
            } => write!(
                f,
                "lightmap pool growth needs {required_layers} array layers, device allows \
                 {max_layers}"
            ),
            Self::Upload(reason) => write!(f, "lightmap drain upload failed: {reason}"),
        }
    }
}

impl std::error::Error for LightmapResidencyDrainError {}

/// What a successful drain changed outside the state itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DrainEffects {
    /// A new generation replaced one pool's live textures: group 4 must
    /// rebind.
    pub(crate) pool_replaced: bool,
    /// The byte meter changed: growth, or a retirement released.
    pub(crate) meter_changed: bool,
}

/// One level's streamed lightmap and mask pools.
///
/// Owned by `LightmapResources` for the level's lifetime. Level unload or
/// reload drops it, and with it each pool's textures, the block table and any
/// retiring generation, whether or not that generation's release has fired:
/// the queue callback holds only a flag.
pub(crate) struct LightmapStreamState {
    model: LightmapPoolModel,
    header: LightmapHeader,
    format: PoolFormat,
    content_tag: [u8; 32],
    max_array_layers: u32,
    /// Generation of the last accepted batch; 0 before the first. One
    /// generation is one controller: a batch under a new one comes from a
    /// fresh controller that holds nothing resident.
    generation: u64,
    /// Highest generation any earlier level install accepted. A batch at or
    /// below it is from a previous level.
    generation_floor: u64,
    /// Each pool's generations, `None` for a pool with nothing to hold: a
    /// mask pool of a level with no id 42, or with no present plane.
    pools: PerPool<Option<PoolGeneration>>,
    table: wgpu::Buffer,
    staging: StagingPool,
    upload_scratch: Vec<u8>,
    failed_scratch: Vec<u32>,
    counters: LightmapStreamCounters,
    /// Each pool whose deferral at the device's layer limit was logged for this
    /// level.
    device_limit_warned: PerPool<bool>,
}

impl LightmapStreamState {
    /// Build the model and each pool's first generation: `layers() + 1` array
    /// layers for a pool that has layers (the spare included, never a second
    /// pool; a pool with none has no generation and no spare), and the whole table,
    /// every real block non-resident and entry 0 the "none" entry.
    pub(crate) fn new(
        device: &wgpu::Device,
        plan: &StreamingPoolPlan,
        generation_floor: u64,
    ) -> Self {
        // The spare layer takes one of the device's array layers. Level data
        // cannot reach the `expect` below: only `plan_streaming_pool` builds a
        // `StreamingPoolPlan`, after its `place_pool` placed these same
        // extents with this same alignment (`block_alignment()` is at least 4,
        // so the model's `max(1)` changes nothing) and refused the level
        // otherwise. `with_layer_limit` fails only when that placement does,
        // and a plane's extent is its block's.
        let model = LightmapPoolModel::with_layer_limit(
            plan.demand.clone(),
            LIGHTMAP_POOL_LAYER_EDGE,
            plan.first_generation_layers,
            plan.max_array_layers.saturating_sub(1),
        )
        .expect("plan_streaming_pool checked that every block fits a pool layer");
        let format = PoolFormat::from_header(&plan.header);
        let pools = PerPool::from_fn(|kind| {
            let pool = model.pool(kind);
            (pool.layers() > 0).then(|| {
                PoolGeneration::new(PoolTextures::new(
                    device,
                    kind,
                    format,
                    pool.spare_layer() + 1,
                ))
            })
        });
        let table_bytes = model.table_bytes();
        // `COPY_SRC` lets a readback check the table the GPU holds.
        let table = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lightmap Block Table (streamed)"),
            contents: &table_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let counters = LightmapStreamCounters {
            pool_texture_sets: PerPool::from_fn(|kind| model.pool(kind).texture_allocations()),
            pool_layers: PerPool::from_fn(|kind| model.pool(kind).layers()),
            active_pool_bytes: PerPool::from_fn(|kind| {
                pools[kind]
                    .as_ref()
                    .map_or(0, |generation| generation.live.bytes())
            }),
            table_entries_written: (table_bytes.len() / BLOCK_TABLE_ENTRY_BYTES) as u64,
            ..LightmapStreamCounters::default()
        };
        log::info!(
            "[Renderer] Lightmap pools (streamed): {} cell block(s), first generation {} lightmap \
             layer(s) and {} mask layer(s) of {}², each pool with layers plus a spare, shadowmask {}",
            plan.demand.block_count(),
            model.pool(PoolKind::Lightmap).layers(),
            model.pool(PoolKind::Mask).layers(),
            LIGHTMAP_POOL_LAYER_EDGE,
            if pools.mask.is_some() {
                "resident"
            } else {
                "placeholder"
            },
        );
        Self {
            model,
            header: plan.header,
            format,
            content_tag: plan.content_tag,
            max_array_layers: plan.max_array_layers,
            generation: 0,
            generation_floor,
            pools,
            table,
            staging: StagingPool::default(),
            upload_scratch: Vec::new(),
            failed_scratch: Vec::new(),
            counters,
            device_limit_warned: PerPool::both(false),
        }
    }

    fn live_texture(&self, plane: PoolPlane) -> Option<&wgpu::Texture> {
        self.pools[plane.pool()]
            .as_ref()
            .map(|generation| generation.live.plane(plane))
    }

    pub(crate) fn irradiance(&self) -> &wgpu::Texture {
        self.live_texture(PoolPlane::Irradiance)
            .expect("a streamed level keeps its lightmap pool")
    }

    pub(crate) fn direction(&self) -> &wgpu::Texture {
        self.live_texture(PoolPlane::Direction)
            .expect("a streamed level keeps its lightmap pool")
    }

    /// The mask pool's live texture, `None` where the placeholder binds.
    pub(crate) fn shadowmask(&self) -> Option<&wgpu::Texture> {
        self.live_texture(PoolPlane::Shadowmask)
    }

    pub(crate) fn table(&self) -> &wgpu::Buffer {
        &self.table
    }

    /// The highest generation this level or an earlier one accepted; the
    /// next level install rejects batches at or below it.
    pub(crate) fn generation_high_water(&self) -> u64 {
        self.generation.max(self.generation_floor)
    }

    /// Both pools' retiring bytes: one pool's release leaves the other's.
    pub(crate) fn retiring_bytes(&self) -> u64 {
        self.counters.retiring_pool_bytes.total()
    }

    pub(crate) fn counters(&self) -> LightmapStreamCounters {
        self.counters
    }

    /// Run one drain: validate, release each pool's completed retirement,
    /// plan, then execute the plan in one encoder and one upload batch (each
    /// pool's growth copies and repack moves through its spare layer, pair
    /// uploads, table writes) and submit it once.
    ///
    /// A pair whose payload does not match its block fails whole: none of its
    /// planes upload and its entry stays non-resident. It is reported in
    /// `failed`, its payload dropped. Deferred pairs are returned owned.
    ///
    /// A batch under a new generation, after the first, comes from a new
    /// controller, which holds nothing resident. Its drain starts from an
    /// empty residency: every placement is freed and every entry turns
    /// non-resident in the same submission, with no eviction reported, while
    /// the textures and any retiring generation stand. The app keeps one
    /// controller per level install where it can; this keeps the pool in
    /// step when it replaces one mid-level.
    pub(crate) fn drain(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        batch: LightmapDrainBatch,
    ) -> Result<(LightmapDrainOutcome, DrainEffects), LightmapResidencyDrainError> {
        batch
            .validate_contract(self.model.block_count(), self.content_tag)
            .map_err(|error| LightmapResidencyDrainError::InvalidBatch(error.to_string()))?;
        self.check_generation(&batch)?;

        let mut effects = DrainEffects::default();
        for kind in PoolKind::ALL {
            let Some(generation) = self.pools[kind].as_mut() else {
                continue;
            };
            if generation.release_completed() {
                self.model.release_retirement(kind);
                self.counters.retiring_pool_bytes[kind] = 0;
                effects.meter_changed = true;
            }
            debug_assert_eq!(
                self.model.pool(kind).retiring(),
                generation.retiring.is_some()
            );
        }

        let started = std::time::Instant::now();
        if self.generation != 0 && batch.generation != self.generation {
            self.model.plan_batch_from_empty(&batch);
        } else {
            self.model.plan_batch(&batch);
        }
        self.fail_mismatched_payloads(&batch);
        let executed = match self.execute(device, queue, &batch.ready) {
            Ok(executed) => executed,
            Err(error) => {
                self.model.abort_drain();
                return Err(error);
            }
        };
        self.generation = batch.generation;
        self.record_drain(executed, started, &mut effects);
        self.warn_device_limit_once();
        Ok((self.outcome(batch), effects))
    }

    /// A deferral at the device limit repeats every drain the same blocks
    /// stay wanted, so it is logged once per pool per level, as the lasting
    /// miss it is; `deferred_pairs` still counts each one.
    fn warn_device_limit_once(&mut self) {
        let limited = PerPool::from_fn(|kind| self.model.plan().pools[kind].device_limited);
        for kind in PoolKind::ALL {
            if !limited[kind] || self.device_limit_warned[kind] {
                continue;
            }
            self.device_limit_warned[kind] = true;
            self.log_device_limit(kind);
        }
    }

    fn log_device_limit(&self, kind: PoolKind) {
        log::warn!(
            "[Renderer] Streamed lightmap: the {} pool holds all {} usable array layer(s) the \
             device allows (maxTextureArrayLayers {}, one kept as the repack spare) and \
             mandatory or visible cell blocks still do not fit; they stay missing, a permanent \
             miss rather than a transient one, until fewer blocks are wanted. Logged once per \
             level",
            kind.name(),
            self.model.pool(kind).layers(),
            self.max_array_layers,
        );
    }

    fn check_generation(
        &self,
        batch: &LightmapDrainBatch,
    ) -> Result<(), LightmapResidencyDrainError> {
        let received = batch.generation;
        let current = self.generation_high_water();
        if received <= self.generation_floor || received < self.generation {
            return Err(LightmapResidencyDrainError::StaleGeneration { current, received });
        }
        if received != self.generation && batch.target_reset.is_none() {
            return Err(LightmapResidencyDrainError::GenerationResetRequired { current, received });
        }
        Ok(())
    }

    /// Fail every planned upload whose payload cannot fill its block, before
    /// anything is recorded, so a pair uploads whole or not at all.
    fn fail_mismatched_payloads(&mut self, batch: &LightmapDrainBatch) {
        let mut failed = std::mem::take(&mut self.failed_scratch);
        failed.clear();
        for upload in &self.model.plan().uploads {
            // A linear scan per upload: the drain's byte budget bounds the
            // batch, so this stays small beside the uploads themselves.
            let reason = match batch.ready.iter().find(|p| p.block == upload.block) {
                Some(prepared) => payload_mismatch(
                    &self.header,
                    (upload.width, upload.height),
                    self.model.planes(upload.block),
                    &prepared.payload,
                ),
                None => Some("no payload arrived".to_string()),
            };
            if let Some(reason) = reason {
                log::error!(
                    "[Renderer] Lightmap block {} pair rejected: {reason}; it stays non-resident",
                    upload.block
                );
                failed.push(upload.block);
            }
        }
        for &block in &failed {
            self.model
                .fail_install(block)
                .expect("a failed block is one of this drain's planned uploads");
        }
        self.failed_scratch = failed;
    }

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
            room_reserved: PerPool {
                lightmap: plan.pools.lightmap.room_reserved.clone(),
                mask: plan.pools.mask.room_reserved.clone(),
            },
            room_returned: PerPool {
                lightmap: plan.pools.lightmap.room_returned.clone(),
                mask: plan.pools.mask.room_returned.clone(),
            },
            pool: plan.report,
        }
    }

    #[cfg(test)]
    pub(crate) fn model(&self) -> &LightmapPoolModel {
        &self.model
    }

    #[cfg(test)]
    fn retiring_pool(&self, kind: PoolKind) -> Option<&textures::RetiringPool> {
        self.pools[kind].as_ref()?.retiring.as_ref()
    }

    #[cfg(test)]
    fn retiring_pool_mut(&mut self, kind: PoolKind) -> Option<&mut textures::RetiringPool> {
        self.pools[kind].as_mut()?.retiring.as_mut()
    }
}
