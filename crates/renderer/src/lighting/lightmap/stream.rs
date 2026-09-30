// Streamed lightmap pool, GPU half: the placement model, the active and
// retiring texture generations, the vertex block table, and each drain.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod counters;
mod execute;
#[cfg(test)]
mod install_timing_measurement;
#[cfg(test)]
mod tests;
mod textures;

use std::fmt;

use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload, LightmapHeader,
};
use postretro_level_format::shadowmask_atlas::group_plane_len;
use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome};
use postretro_render_cpu::lightmap_pool::{BLOCK_TABLE_ENTRY_BYTES, LightmapPoolModel};
use wgpu::util::DeviceExt;

pub use counters::LightmapStreamCounters;
pub(crate) use textures::PoolTextures;
use textures::RetiringPool;

use super::StreamingPoolPlan;
use super::pool::PoolFormat;
use crate::render::StagingPool;

/// A drain the renderer refused. Every variant leaves the pool, the table
/// and the model exactly as they were before the drain.
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
    /// A new generation replaced the active textures: group 4 must rebind.
    pub(crate) pool_replaced: bool,
    /// The byte meter changed: growth, or a retirement released.
    pub(crate) meter_changed: bool,
}

/// One level's streamed lightmap pool.
///
/// Owned by `LightmapResources` for the level's lifetime. Level unload or
/// reload drops it, and with it the active textures, the block table and any
/// retiring generation, whether or not that generation's release has fired:
/// the queue callback holds only a flag.
pub(crate) struct LightmapStreamState {
    model: LightmapPoolModel,
    header: LightmapHeader,
    format: PoolFormat,
    with_shadowmask: bool,
    content_tag: [u8; 32],
    max_array_layers: u32,
    /// Generation of the last accepted batch; 0 before the first. One
    /// generation is one controller: a batch under a new one comes from a
    /// fresh controller that holds nothing resident.
    generation: u64,
    /// Highest generation any earlier level install accepted. A batch at or
    /// below it is from a previous level.
    generation_floor: u64,
    textures: PoolTextures,
    retiring: Option<RetiringPool>,
    table: wgpu::Buffer,
    staging: StagingPool,
    upload_scratch: Vec<u8>,
    failed_scratch: Vec<u32>,
    counters: LightmapStreamCounters,
    /// A deferral at the device's layer limit was logged for this level.
    device_limit_warned: bool,
}

impl LightmapStreamState {
    /// Build the model and the first generation: `layers() + 1` array layers
    /// (the spare included, never a second pool) and the whole table, every
    /// real block non-resident and entry 0 the "none" entry.
    pub(crate) fn new(
        device: &wgpu::Device,
        plan: &StreamingPoolPlan,
        generation_floor: u64,
    ) -> Self {
        // The spare layer takes one of the device's array layers.
        let model = LightmapPoolModel::with_layer_limit(
            plan.extents.clone(),
            plan.header.block_alignment(),
            LIGHTMAP_POOL_LAYER_EDGE,
            plan.pool_cap_layers,
            plan.max_array_layers.saturating_sub(1),
        )
        .expect("plan_streaming_pool checked that every block fits a pool layer");
        let format = PoolFormat::from_header(&plan.header);
        let textures = PoolTextures::new(
            device,
            format,
            model.spare_layer() + 1,
            plan.with_shadowmask,
        );
        let table_bytes = model.table_bytes();
        // `COPY_SRC` lets a readback check the table the GPU holds.
        let table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lightmap Block Table (streamed)"),
            contents: &table_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let counters = LightmapStreamCounters {
            pool_texture_sets: 1,
            pool_layers: model.layers(),
            active_pool_bytes: textures.bytes(),
            table_entries_written: (table_bytes.len() / BLOCK_TABLE_ENTRY_BYTES) as u64,
            ..LightmapStreamCounters::default()
        };
        log::info!(
            "[Renderer] Lightmap pool (streamed): {} cell block(s), first generation {} layer(s) \
             of {}² plus a spare, shadowmask {}",
            plan.extents.len(),
            model.layers(),
            LIGHTMAP_POOL_LAYER_EDGE,
            if plan.with_shadowmask {
                "resident"
            } else {
                "placeholder"
            },
        );
        Self {
            model,
            header: plan.header,
            format,
            with_shadowmask: plan.with_shadowmask,
            content_tag: plan.content_tag,
            max_array_layers: plan.max_array_layers,
            generation: 0,
            generation_floor,
            textures,
            retiring: None,
            table,
            staging: StagingPool::default(),
            upload_scratch: Vec::new(),
            failed_scratch: Vec::new(),
            counters,
            device_limit_warned: false,
        }
    }

    pub(crate) fn textures(&self) -> &PoolTextures {
        &self.textures
    }

    pub(crate) fn table(&self) -> &wgpu::Buffer {
        &self.table
    }

    /// The highest generation this level or an earlier one accepted; the
    /// next level install rejects batches at or below it.
    pub(crate) fn generation_high_water(&self) -> u64 {
        self.generation.max(self.generation_floor)
    }

    pub(crate) fn retiring_bytes(&self) -> u64 {
        self.retiring.as_ref().map_or(0, RetiringPool::bytes)
    }

    pub(crate) fn counters(&self) -> LightmapStreamCounters {
        self.counters
    }

    /// Run one drain: validate, release a completed retirement, plan, then
    /// execute the plan in one encoder and one upload batch (growth copies,
    /// repack moves through the spare layer, pair uploads, table writes)
    /// and submit it once.
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
        queue: &wgpu::Queue,
        batch: LightmapDrainBatch,
    ) -> Result<(LightmapDrainOutcome, DrainEffects), LightmapResidencyDrainError> {
        batch
            .validate_contract(self.model.block_count(), self.content_tag)
            .map_err(|error| LightmapResidencyDrainError::InvalidBatch(error.to_string()))?;
        self.check_generation(&batch)?;

        let mut effects = DrainEffects::default();
        if self
            .retiring
            .as_ref()
            .is_some_and(RetiringPool::is_complete)
        {
            self.retiring = None;
            self.model.release_retirement();
            self.counters.retiring_pool_bytes = 0;
            effects.meter_changed = true;
        }
        debug_assert_eq!(self.model.retiring(), self.retiring.is_some());

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
    /// stay wanted, so it is logged once per level, as the lasting miss it
    /// is; `deferred_pairs` still counts each one.
    fn warn_device_limit_once(&mut self) {
        if !self.model.plan().device_limited || self.device_limit_warned {
            return;
        }
        self.device_limit_warned = true;
        log::warn!(
            "[Renderer] Streamed lightmap: the pool holds all {} usable array layer(s) the \
             device allows (maxTextureArrayLayers {}, one kept as the repack spare) and \
             mandatory or visible cell blocks still do not fit; they stay missing, a permanent \
             miss rather than a transient one, until fewer blocks are wanted. Logged once per \
             level",
            self.model.layers(),
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
                    &prepared.payload,
                    self.with_shadowmask,
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
            pool: plan.report,
        }
    }

    #[cfg(test)]
    pub(crate) fn model(&self) -> &LightmapPoolModel {
        &self.model
    }

    #[cfg(test)]
    fn retiring_pool(&self) -> Option<&RetiringPool> {
        self.retiring.as_ref()
    }
}

/// Why `payload` cannot fill a `width × height` block, or `None` when every
/// plane the pool keeps matches the block's extent and format.
fn payload_mismatch(
    header: &LightmapHeader,
    (width, height): (u32, u32),
    payload: &LightmapBlockPayload,
    with_shadowmask: bool,
) -> Option<String> {
    let irradiance = header.irradiance_len(width, height);
    let direction = header.direction_len(width, height);
    if irradiance != Some(payload.irradiance.len() as u64)
        || direction != Some(payload.direction.len() as u64)
    {
        return Some(format!(
            "{width}x{height} block carries {} irradiance and {} direction byte(s), expected \
             {irradiance:?} and {direction:?}",
            payload.irradiance.len(),
            payload.direction.len(),
        ));
    }
    if !with_shadowmask {
        return None;
    }
    let expected = group_plane_len(width, height);
    match &payload.shadowmask {
        Some([a, b]) if expected == Some(a.len() as u64) && expected == Some(b.len() as u64) => {
            None
        }
        Some([a, b]) => Some(format!(
            "shadowmask groups are {} and {} byte(s), expected {expected:?}",
            a.len(),
            b.len()
        )),
        None => Some("the shadowmask groups are missing".to_string()),
    }
}
