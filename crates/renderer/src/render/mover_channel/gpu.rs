// Mover probe channel GPU tables: the direct table both direct-SH compose
// passes bind at binding 25, the bounce table the indirect SH compose binds
// at its binding 25, each table's per-frame t′ block, and the per-cell
// residency of a streamed payload.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting, Cluster SH
// residency)
//
// A resident block keeps a whole table: every cell's payload at a fixed base,
// written at load (or, for direct cells that borrow a streamed id-41/45
// term, once by the first cluster that carries it). A streamed block keeps a
// pooled table: a cell's payload is placed when the first resident cluster
// carrying it installs and freed when the last one leaves, and its base word
// reads "not resident" in between. That is residency, never a light- or
// mover-driven switch.

use postretro_level_format::mover_probe_channel::{
    MoverCellSlices, MoverChannelBlock, MoverPayloadPlacement, MoverProbeChannelSection,
    MoverStreamedRow,
};

use super::bounce::BounceLayout;
use super::depth::{DepthBases, MoverDepthTiles, ReferenceChange};
use super::layout::{
    CellWrite, MoverChannelError, MoverChannelInputs, MoverChannelLayout, MoverChannelRows,
    NO_BASE, TermRow, check_direct_cell,
};
use super::pool::CellPool;
use super::state::{LegacyPass, MoverTBlock};
use crate::render::MoverTravel;
use crate::render::gpu_create::GpuCreate;
use crate::render::residency::{ResidencyAllocationState, source_ids};
use crate::render::sh_allocation::{ShAllocationKind, buffer_allocation};
use crate::render::sh_residency::ShAllocationLedger;
use crate::render::uploads::{StagedUploads, UploadError, UploadQueue};

/// One streamed sparse row of id 41 or id 45: its affinity cell, each
/// entry's first f16 half in `tiles`, and the tiles.
#[derive(Clone, Copy, Debug)]
pub(in crate::render) struct ClusterSparseRow<'a> {
    pub(in crate::render) cell: u32,
    pub(in crate::render) entry_offsets: &'a [u32],
    pub(in crate::render) tiles: &'a [u16],
}

/// What one cluster brings that the channel reads: its id-41 and id-45 rows
/// and its id-57 rows.
#[derive(Default)]
pub(in crate::render) struct ClusterTermRows<'a> {
    pub(in crate::render) static_rows: Vec<ClusterSparseRow<'a>>,
    pub(in crate::render) animated_rows: Vec<ClusterSparseRow<'a>>,
    pub(in crate::render) channel_rows: Vec<MoverStreamedRow<'a>>,
}

fn find_row<'a>(rows: &[ClusterSparseRow<'a>], cell: u32, valid: u64) -> Option<TermRow<'a>> {
    rows.iter().find(|row| row.cell == cell).map(|row| TermRow {
        entry_offsets: row.entry_offsets,
        tiles: row.tiles,
        valid_probe_mask: valid,
    })
}

/// The two tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableKind {
    Direct,
    Bounce,
}

/// One cell payload a cluster places or completes.
#[derive(Debug)]
struct PlacedWrite {
    table: TableKind,
    plan: usize,
    /// Absolute word the payload starts at.
    base: u32,
    words: Vec<u32>,
    /// Pooled: the base word is published after the payload.
    pooled: bool,
}

/// One cluster's channel work, staged before its upload batch.
#[derive(Debug, Default)]
pub(in crate::render) struct ClusterChannelWrites {
    writes: Vec<PlacedWrite>,
    /// A pooled table grew to hold these cells: every compose pass binding
    /// it rebinds.
    pub(in crate::render) grew: bool,
}

/// One GPU table.
struct ChannelTable {
    label: &'static str,
    buffer: wgpu::Buffer,
    /// Words before the payload region: the table's fixed part.
    payload_word: usize,
    /// `Some` while the block streams: cells placed per resident cluster.
    pool: Option<CellPool>,
}

impl ChannelTable {
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        words: &[u32],
        payload_word: usize,
        pool: Option<CellPool>,
    ) -> Self {
        Self {
            label,
            buffer: table_buffer(device, label, words, 0),
            payload_word,
            pool,
        }
    }

    /// Bytes the pool holds now, and the bytes its resident cells fill.
    fn pool_bytes(&self) -> (u64, u64) {
        self.pool.as_ref().map_or((0, 0), |pool| {
            (u64::from(pool.capacity_words()) * 4, pool.live_words() * 4)
        })
    }

    /// Size a pool nothing has been placed in yet: recreate the table with
    /// its fixed words and `words` of pool.
    fn size_empty_pool(&mut self, device: &wgpu::Device, fixed: &[u32], words: u32) {
        let Some(pool) = self.pool.as_mut() else {
            return;
        };
        pool.set_capacity_words(words);
        self.buffer = table_buffer(device, self.label, fixed, words);
    }

    /// Grow a live pool to hold every placed range: a new table, the old
    /// one's words copied in on the queue ahead of any later write.
    fn grow_pool(&mut self, device: &wgpu::Device, queue: &UploadQueue) -> bool {
        let Some(pool) = self.pool.as_mut() else {
            return false;
        };
        if pool.needed_words() <= pool.capacity_words() {
            return false;
        }
        let words = pool.grown_words();
        let old_bytes = pool.capacity_words() as u64 * 4;
        log::warn!(
            "[Renderer] MoverProbeChannel: {} pool grows from {old_bytes} B to {} B for the cells resident now",
            self.label,
            u64::from(words) * 4
        );
        pool.set_capacity_words(words);
        let replacement = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: (self.payload_word as u64 + u64::from(words)) * 4,
            usage: TABLE_USAGE,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Mover Probe Channel Pool Growth Copy"),
        });
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &replacement, 0, self.buffer.size());
        queue.submit(std::iter::once(encoder.finish()));
        self.buffer = replacement;
        true
    }
}

const TABLE_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE
    .union(wgpu::BufferUsages::COPY_DST)
    .union(wgpu::BufferUsages::COPY_SRC);

/// A table of `words` then `pool_words` of zeroed pool.
fn table_buffer(
    device: &wgpu::Device,
    label: &'static str,
    words: &[u32],
    pool_words: u32,
) -> wgpu::Buffer {
    let mut contents: Vec<u8> = bytemuck::cast_slice(words).to_vec();
    contents.resize(contents.len() + pool_words as usize * 4, 0);
    device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: &contents,
        usage: TABLE_USAGE,
    })
}

/// Id 57's structure a streamed level keeps to write cells as clusters
/// arrive: indexes, records, masks, mover entries and ramps, and each
/// resident block's payload. Bounce entries stay only while the bounce
/// payload streams; the depth block is never kept here.
#[derive(Debug)]
struct StreamedCells {
    section: MoverProbeChannelSection,
    slices: Vec<MoverCellSlices>,
    /// A whole direct table on a streamed level: which cells are written.
    /// Each is written once, by the first cluster carrying its terms.
    direct_written: Option<Vec<bool>>,
}

pub(in crate::render) struct MoverChannelResources {
    direct: ChannelTable,
    bounce: ChannelTable,
    layout: MoverChannelLayout,
    bounce_layout: BounceLayout,
    rows: MoverChannelRows,
    streamed: Option<StreamedCells>,
    t: MoverTBlock,
    depth: MoverDepthTiles,
}

impl MoverChannelResources {
    /// Lay out both tables and write every cell the load already holds: a
    /// whole load writes them all from `whole_rows` (cell, is-id-45 → that
    /// family's row); a streamed one only resident-block cells that borrow
    /// no streamed term, the rest arriving with their clusters.
    pub(in crate::render) fn new<'a>(
        device: &wgpu::Device,
        inputs: &MoverChannelInputs<'_>,
        streaming: bool,
        whole_rows: impl FnMut(u32, bool) -> Option<TermRow<'a>>,
        ledger: &mut ShAllocationLedger,
    ) -> Result<Self, MoverChannelError> {
        let pooled = |block: MoverChannelBlock| {
            streaming
                && inputs.section.is_some_and(|section| {
                    section.placement(block) == MoverPayloadPlacement::Streamed
                })
        };
        let (pooled_direct, pooled_bounce) = (
            pooled(MoverChannelBlock::Direct),
            pooled(MoverChannelBlock::Bounce),
        );
        let layout = MoverChannelLayout::build(inputs, pooled_direct)?;
        let bounce_layout = BounceLayout::build(
            inputs.section,
            inputs.indirect_descriptor_indices,
            layout.mover_ids(),
            pooled_bounce,
        )?;
        let mut direct_words = layout.initial_words().to_vec();
        let mut bounce_words = bounce_layout.initial_words().to_vec();
        let mut streamed = None;
        if let Some(section) = inputs.section {
            if streaming {
                let kept = structure(section);
                let slices = kept.cell_slices();
                let mut direct_written = None;
                if !pooled_direct {
                    // Cells that borrow a streamed term or payload write as
                    // their clusters arrive.
                    let writes = layout.held_writes(&kept, &slices, |_, _| None)?;
                    let mut written = vec![false; layout.cell_count()];
                    for write in &writes {
                        apply(&mut direct_words, layout.whole_base(write.plan), write);
                        written[write.plan] = true;
                    }
                    direct_written = written.iter().any(|written| !written).then_some(written);
                }
                if !pooled_bounce {
                    write_whole_bounce(&bounce_layout, section, &mut bounce_words)?;
                }
                streamed = Some(StreamedCells {
                    section: kept,
                    slices,
                    direct_written,
                });
            } else {
                let slices = section.cell_slices();
                let writes = layout.held_writes(section, &slices, whole_rows)?;
                if writes.len() != layout.cell_count() {
                    return Err(MoverChannelError(format!(
                        "a whole load writes {} of {} gated cell(s); the rest lack their payload or terms",
                        writes.len(),
                        layout.cell_count()
                    )));
                }
                for write in &writes {
                    apply(&mut direct_words, layout.whole_base(write.plan), write);
                }
                write_whole_bounce(&bounce_layout, section, &mut bounce_words)?;
            }
            if layout.ungated_records() > 0 {
                log::warn!(
                    "[Renderer] MoverProbeChannel: {} gated term record(s) compose ungated: their DirectShDeltaVolumes (id 41) or AnimatedDirectShDeltaVolumes (id 45) is absent or disabled at load",
                    layout.ungated_records()
                );
            }
            log::info!(
                "[Renderer] MoverProbeChannel: {} direct record(s) over {} cell(s), {} bounce cell(s), {} mover(s); direct table {} B{}, bounce table {} B{}",
                layout.record_count(),
                layout.cell_count(),
                bounce_layout.cell_count(),
                layout.mover_ids().len(),
                direct_words.len() * 4,
                if pooled_direct { " + pool" } else { "" },
                bounce_words.len() * 4,
                if pooled_bounce { " + pool" } else { "" },
            );
        }
        let present = inputs.section.is_some();
        record_table(
            ledger,
            ShAllocationKind::MoverChannelTable,
            &direct_words,
            [
                present.then_some(57),
                (present && inputs.static_terms.is_some()).then_some(41),
                inputs.animated_terms.is_some().then_some(45),
            ],
        );
        record_table(
            ledger,
            ShAllocationKind::MoverBounceTable,
            &bounce_words,
            [
                (present && bounce_layout.cell_count() > 0).then_some(57),
                (!inputs.indirect_descriptor_indices.is_empty()).then_some(27),
                None,
            ],
        );
        let depth = MoverDepthTiles::new(inputs.section, layout.mover_ids())?;
        let mut t = MoverTBlock::new(layout.mover_ids());
        let mut rows = layout.rows().clone();
        rows.bounce_by_mover = bounce_layout.rows_by_mover().clone();
        t.set_feeds(LegacyPass::Direct, |slot| {
            rows.static_by_mover.has_rows(slot) || rows.animated_by_mover.has_rows(slot)
        });
        t.set_feeds(LegacyPass::Indirect, |slot| {
            rows.bounce_by_mover.has_rows(slot)
        });
        let direct = ChannelTable::new(
            device,
            "Mover Probe Channel Table",
            &direct_words,
            layout.payload_word(),
            pooled_direct.then(|| CellPool::new(layout.cell_payload_words())),
        );
        let bounce = ChannelTable::new(
            device,
            "Mover Probe Channel Bounce Table",
            &bounce_words,
            bounce_layout.payload_word(),
            pooled_bounce.then(|| CellPool::new(bounce_layout.cell_payload_words())),
        );
        Ok(Self {
            direct,
            bounce,
            layout,
            bounce_layout,
            rows,
            streamed,
            t,
            depth,
        })
    }

    /// No level: both table headers, no descriptors.
    pub(in crate::render) fn empty(device: &wgpu::Device) -> Self {
        let layout = MoverChannelLayout::empty(&[]);
        let bounce_layout =
            BounceLayout::build(None, &[], &[], false).expect("an empty bounce table lays out");
        Self {
            direct: ChannelTable::new(
                device,
                "Mover Probe Channel Table (empty)",
                layout.initial_words(),
                layout.payload_word(),
                None,
            ),
            bounce: ChannelTable::new(
                device,
                "Mover Probe Channel Bounce Table (empty)",
                bounce_layout.initial_words(),
                bounce_layout.payload_word(),
                None,
            ),
            layout,
            bounce_layout,
            rows: MoverChannelRows::default(),
            streamed: None,
            t: MoverTBlock::new(&[]),
            depth: MoverDepthTiles::default(),
        }
    }

    /// The direct table both direct compose passes bind.
    pub(in crate::render) fn buffer(&self) -> &wgpu::Buffer {
        &self.direct.buffer
    }

    /// The bounce table the indirect compose binds at binding 25.
    pub(in crate::render) fn bounce_buffer(&self) -> &wgpu::Buffer {
        &self.bounce.buffer
    }

    /// The direct and bounce tables the whole-load composes bind. A pooled
    /// table grows by replacement and only the streamed passes rebind to the
    /// new one. A pooled table means a streamed level, whose whole-load
    /// composes never dispatch, so they bind an empty table in its place
    /// and never hold a grown pool's old buffer until unload. Each empty
    /// table is a small header buffer of its own, recorded in `ledger` as a
    /// dummy row of its table's kind.
    pub(in crate::render) fn whole_load_tables(
        &self,
        device: &wgpu::Device,
        ledger: &mut ShAllocationLedger,
    ) -> [wgpu::Buffer; 2] {
        let mut stand_in = |label: &'static str, kind: ShAllocationKind, words: &[u32]| {
            record_table(ledger, kind, words, [None; 3]);
            table_buffer(device, label, words, 0)
        };
        let direct = match self.direct.pool {
            None => self.direct.buffer.clone(),
            Some(_) => stand_in(
                "Mover Probe Channel Table (whole-load, streamed level)",
                ShAllocationKind::MoverChannelTable,
                MoverChannelLayout::empty(&[]).initial_words(),
            ),
        };
        let bounce = match self.bounce.pool {
            None => self.bounce.buffer.clone(),
            Some(_) => stand_in(
                "Mover Probe Channel Bounce Table (whole-load, streamed level)",
                ShAllocationKind::MoverBounceTable,
                BounceLayout::build(None, &[], &[], false)
                    .expect("an empty bounce table lays out")
                    .initial_words(),
            ),
        };
        [direct, bounce]
    }

    pub(in crate::render) fn rows(&self) -> &MoverChannelRows {
        &self.rows
    }

    /// Whether the channel gates any direct-SH term: a streamed level then
    /// runs its direct passes even with no id 41 or id 45.
    pub(in crate::render) fn gates_direct(&self) -> bool {
        self.layout.record_count() > 0
    }

    /// Take this frame's mover states; idempotent within a frame.
    pub(in crate::render) fn set_states(&mut self, states: &[MoverTravel]) {
        self.t.set_states(states);
    }

    /// Write the t′ blocks when they moved. Staged writes precede the
    /// frame's first submission, so compose recorded this frame reads this
    /// frame's t′.
    pub(in crate::render) fn upload_t(&mut self, queue: &UploadQueue) {
        if let Some(bytes) = self.t.take_upload() {
            queue.write_buffer(&self.direct.buffer, self.layout.t_byte_offset(), bytes);
            queue.write_buffer(
                &self.bounce.buffer,
                self.bounce_layout.t_byte_offset(),
                bytes,
            );
        }
    }

    /// Live tiles the depth block needs after id 59's.
    pub(in crate::render) fn depth_live_tile_count(&self) -> u32 {
        self.depth.live_tile_count()
    }

    /// Closed-pose tile bytes held on the CPU.
    pub(in crate::render) fn depth_host_bytes(&self) -> u64 {
        self.depth.host_bytes()
    }

    /// Drop the depth block on a level with no usable SH: nothing samples
    /// depth, so no closed-pose tile is held and no frame plans a write.
    pub(in crate::render) fn discard_depth(&mut self) {
        self.depth = MoverDepthTiles::default();
    }

    /// Bind the depth block's live tiles to the installed tile texture;
    /// `bases` gives the moment set an unstored terminus blends toward.
    pub(in crate::render) fn bind_depth(
        &mut self,
        first_live_tile: u32,
        layout: postretro_level_format::probe_depth_tiles::ProbeDepthTileTextureLayout,
        references: &postretro_render_cpu::sh_depth_tiles::ProbeDepthTileReferences,
        bases: DepthBases<'_>,
    ) {
        self.depth.bind(first_live_tile, layout, references, bases);
    }

    /// Plan this frame's live tiles from the held t′: every tile a mover
    /// whose t′ moved since the last committed write covers reaches
    /// `write_run`, and the probe references to move return. Call once per
    /// frame, after [`Self::set_states`], then [`Self::commit_depth`] once
    /// every reference move landed or [`Self::abandon_depth`] if one failed,
    /// so a failed frame's change is planned again. A frame allocates
    /// nothing.
    pub(in crate::render) fn plan_depth(
        &mut self,
        write_run: impl FnMut(u32, u32, &[u8]),
    ) -> &[ReferenceChange] {
        let (values, changed) = self.t.depth_changes();
        self.depth.plan(values, changed, write_run)
    }

    /// The last [`Self::plan_depth`]'s writes and reference moves landed.
    pub(in crate::render) fn commit_depth(&mut self) {
        self.t.commit_depth_changes();
    }

    /// A reference move of the last [`Self::plan_depth`] failed: keep its
    /// mover changes for the next frame.
    pub(in crate::render) fn abandon_depth(&mut self) {
        self.depth.abandon_plan();
    }

    /// Live tiles the last planned frame wrote.
    #[cfg(test)]
    pub(in crate::render) fn depth_tiles_written(&self) -> u32 {
        self.depth.tiles_written()
    }

    /// Slots whose t′ moved since the streamed planner last committed an
    /// observation. Peeking marks nothing: call
    /// [`Self::commit_t_changes`] once the frame's plan succeeded.
    pub(in crate::render) fn observe_t_changes(&mut self) -> &[u32] {
        self.t.observe_changes()
    }

    /// The streamed planner staled the rows of the last observed changes.
    pub(in crate::render) fn commit_t_changes(&mut self) {
        self.t.commit_observed();
    }

    /// The legacy direct compose's mover-state trigger.
    pub(in crate::render) fn t_changed_since_composed(&self) -> bool {
        self.t.changed_since_composed(LegacyPass::Direct)
    }

    /// The legacy direct compose encoded this frame's t′: commit its trigger.
    pub(in crate::render) fn commit_composed_t(&mut self) {
        self.t.commit_composed(LegacyPass::Direct);
    }

    /// The legacy indirect compose's mover-state trigger: only movers with
    /// bounce entries fire it.
    pub(in crate::render) fn bounce_t_changed_since_composed(&self) -> bool {
        self.t.changed_since_composed(LegacyPass::Indirect)
    }

    /// The legacy indirect compose encoded this frame's t′.
    pub(in crate::render) fn commit_composed_bounce_t(&mut self) {
        self.t.commit_composed(LegacyPass::Indirect);
    }

    /// Whether a streamed cluster can bring this table anything: a whole
    /// direct cell still waiting for its terms, or a pooled payload.
    pub(in crate::render) fn has_streamed_cells(&self) -> bool {
        self.streamed.as_ref().is_some_and(|streamed| {
            streamed.direct_written.is_some()
                || self.direct.pool.is_some()
                || self.bounce.pool.is_some()
        })
    }

    /// Whether a cluster bringing `first_cells` first can complete a direct
    /// cell from its id-41/45 rows: a whole cell still waiting for its
    /// borrowed terms, or a pooled direct cell it places.
    pub(in crate::render) fn wants_terms(&self, first_cells: &[u32]) -> bool {
        self.streamed.as_ref().is_some_and(|streamed| {
            streamed.direct_written.is_some()
                || (self.direct.pool.is_some()
                    && first_cells
                        .iter()
                        .any(|&cell| self.layout.plan_of_cell(cell).is_some()))
        })
    }

    /// Whether listed cell `cell` places a payload per resident cluster in
    /// either table.
    pub(in crate::render) fn pools_cell(&self, cell: u32) -> bool {
        (self.direct.pool.is_some() && self.layout.plan_of_cell(cell).is_some())
            || (self.bounce.pool.is_some() && self.bounce_layout.plan_of_cell(cell).is_some())
    }

    /// Size each streamed block's pool for a level whose dense SH pool
    /// starts at `dense_fraction` of the map. Runs once at streamed install,
    /// before any compose pass binds a table.
    pub(in crate::render) fn size_streamed_pools(
        &mut self,
        device: &wgpu::Device,
        dense_fraction: f64,
    ) {
        for (table, fixed) in [
            (&mut self.direct, self.layout.initial_words()),
            (&mut self.bounce, self.bounce_layout.initial_words()),
        ] {
            let Some(words) = table
                .pool
                .as_ref()
                .map(|pool| pool.initial_words(dense_fraction))
            else {
                continue;
            };
            table.size_empty_pool(device, fixed, words);
            log::info!(
                "[Renderer] MoverProbeChannel: {} pool starts at {} B ({:.0}% of the dense SH map resident)",
                table.label,
                u64::from(words) * 4,
                dense_fraction.min(1.0) * 100.0
            );
        }
    }

    /// Pool bytes allocated now and bytes resident cells fill, over both
    /// tables: the channel's share of the streamed SH pool.
    pub(in crate::render) fn pool_bytes(&self) -> (u64, u64) {
        let (direct, bounce) = (self.direct.pool_bytes(), self.bounce.pool_bytes());
        (direct.0 + bounce.0, direct.1 + bounce.1)
    }

    /// The writes one streamed cluster brings: each pooled cell `first_cells`
    /// names (cells whose first resident cluster this is) placed from the
    /// cluster's id-57 rows, and each whole direct cell still waiting that
    /// the cluster's rows complete. Every arriving row's direct payload is
    /// checked as the whole load checks a held one, every record gated or
    /// not, and each placed bounce cell for finite tiles, so a malformed row
    /// refuses the cluster before any upload. A pool that cannot hold its
    /// cells grows here.
    pub(in crate::render) fn stage_cluster(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        rows: &ClusterTermRows<'_>,
        first_cells: &[u32],
    ) -> Result<ClusterChannelWrites, MoverChannelError> {
        let mut out = ClusterChannelWrites::default();
        let result = self.place_cluster(rows, first_cells, &mut out);
        if let Err(error) = result {
            self.undo_cluster(&out);
            return Err(error);
        }
        let mut grew = false;
        for table in [&mut self.direct, &mut self.bounce] {
            grew |= table.grow_pool(device, queue);
        }
        out.grew = grew;
        Ok(out)
    }

    fn place_cluster(
        &mut self,
        rows: &ClusterTermRows<'_>,
        first_cells: &[u32],
        out: &mut ClusterChannelWrites,
    ) -> Result<(), MoverChannelError> {
        let Some(streamed) = self.streamed.as_ref() else {
            return Ok(());
        };
        // Every arriving row's direct payload is checked whole, each record
        // whether the table gates it or not, as the whole load checks it at
        // attach: the same PRL is accepted or refused alike in every mode.
        if !streamed.section.payload_held(MoverChannelBlock::Direct) {
            for row in &rows.channel_rows {
                let Some(position) = streamed.section.listed_position(row.cell as usize) else {
                    return Err(MoverChannelError(format!(
                        "a streamed row names affinity cell {}, which the section does not list",
                        row.cell
                    )));
                };
                let payload = streamed
                    .section
                    .cell_payload(position, &streamed.slices[position], Some(*row))
                    .map_err(|error| MoverChannelError(error.to_string()))?;
                check_direct_cell(&streamed.section, position, payload)?;
            }
        }
        let channel_row = |cell: u32| {
            rows.channel_rows
                .iter()
                .find(|row| row.cell == cell)
                .copied()
        };
        let terms = |plan: usize, cell: u32| {
            let (static_valid, animated_valid) = self.layout.plan_valid_masks(plan);
            (
                find_row(&rows.static_rows, cell, static_valid),
                find_row(&rows.animated_rows, cell, animated_valid),
            )
        };
        let direct_write = |plan: usize, cell: u32| -> Result<CellWrite, MoverChannelError> {
            let (needs_static, needs_animated) = self.layout.plan_needs(plan);
            let (static_row, animated_row) = terms(plan, cell);
            let streamed_payload = !streamed.section.payload_held(MoverChannelBlock::Direct);
            let row = channel_row(cell);
            if (streamed_payload && row.is_none())
                || (needs_static && static_row.is_none())
                || (needs_animated && animated_row.is_none())
            {
                return Err(MoverChannelError(format!(
                    "a cluster brings part of affinity cell {cell}'s gated terms but not all of them"
                )));
            }
            let payload =
                self.layout
                    .plan_payload(&streamed.section, &streamed.slices, plan, row)?;
            self.layout
                .cell_write(&streamed.section, plan, payload, static_row, animated_row)
        };

        // A whole direct table's cells waiting for a borrowed term.
        if let Some(written) = streamed.direct_written.as_ref() {
            let mut cells: Vec<u32> = rows
                .static_rows
                .iter()
                .map(|row| row.cell)
                .chain(rows.animated_rows.iter().map(|row| row.cell))
                .chain(rows.channel_rows.iter().map(|row| row.cell))
                .collect();
            cells.sort_unstable();
            cells.dedup();
            for cell in cells {
                let Some(plan) = self.layout.plan_of_cell(cell) else {
                    continue;
                };
                if written[plan] {
                    continue;
                }
                let write = direct_write(plan, cell)?;
                out.writes.push(PlacedWrite {
                    table: TableKind::Direct,
                    plan,
                    base: self.layout.whole_base(plan).unwrap_or(NO_BASE),
                    words: write.words,
                    pooled: false,
                });
            }
        }

        // Pooled cells this cluster brings first.
        let mut placed = Vec::new();
        for &cell in first_cells {
            if self.direct.pool.is_some()
                && let Some(plan) = self.layout.plan_of_cell(cell)
            {
                placed.push((TableKind::Direct, plan, direct_write(plan, cell)?.words));
            }
            if self.bounce.pool.is_some()
                && let Some(plan) = self.bounce_layout.plan_of_cell(cell)
            {
                let position = self.bounce_layout.position(plan);
                let payload = streamed
                    .section
                    .cell_payload(position, &streamed.slices[position], channel_row(cell))
                    .map_err(|error| MoverChannelError(error.to_string()))?;
                let write = self
                    .bounce_layout
                    .cell_write(&streamed.section, plan, payload)?;
                placed.push((TableKind::Bounce, plan, write.words));
            }
        }
        for (table, plan, words) in placed {
            let (pool, payload_word) = match table {
                TableKind::Direct => (self.direct.pool.as_mut(), self.direct.payload_word),
                TableKind::Bounce => (self.bounce.pool.as_mut(), self.bounce.payload_word),
            };
            let range = pool
                .and_then(|pool| pool.place(plan))
                .ok_or_else(|| MoverChannelError(format!("cell plan {plan} cannot place")))?;
            debug_assert_eq!(range.len as usize, words.len());
            out.writes.push(PlacedWrite {
                table,
                plan,
                base: payload_word as u32 + range.start,
                words,
                pooled: true,
            });
        }
        Ok(())
    }

    /// Free what `writes` placed: a later step of the install failed.
    pub(in crate::render) fn undo_cluster(&mut self, writes: &ClusterChannelWrites) {
        for write in writes.writes.iter().filter(|write| write.pooled) {
            if let Some(pool) = self.table_mut(write.table).pool.as_mut() {
                pool.release(write.plan);
            }
        }
    }

    /// Stage `writes` into a cluster's upload batch: each payload, then a
    /// pooled cell's base word, so no base points at unwritten words.
    pub(in crate::render) fn stage_writes(
        &self,
        uploads: &mut StagedUploads,
        writes: &ClusterChannelWrites,
    ) -> Result<(), UploadError> {
        for write in &writes.writes {
            let table = self.table(write.table);
            uploads.write_buffer(
                &table.buffer,
                u64::from(write.base) * 4,
                bytemuck::cast_slice(&write.words),
            )?;
        }
        for write in writes.writes.iter().filter(|write| write.pooled) {
            let offset = match write.table {
                TableKind::Direct => self.layout.base_byte_offset(write.plan),
                TableKind::Bounce => self.bounce_layout.base_byte_offset(write.plan),
            };
            uploads.write_buffer(
                &self.table(write.table).buffer,
                offset,
                &write.base.to_le_bytes(),
            )?;
        }
        Ok(())
    }

    /// The cluster's writes were submitted: a whole table's cells never
    /// rewrite.
    pub(in crate::render) fn commit_cluster(&mut self, writes: &ClusterChannelWrites) {
        let Some(streamed) = self.streamed.as_mut() else {
            return;
        };
        let Some(written) = streamed.direct_written.as_mut() else {
            return;
        };
        for write in writes.writes.iter().filter(|write| !write.pooled) {
            written[write.plan] = true;
        }
        if written.iter().all(|written| *written) {
            streamed.direct_written = None;
        }
    }

    /// The last resident cluster carrying listed cell `cell` left: free its
    /// pooled payloads and mark them not resident, in `uploads`, which is
    /// submitted before any later install can reuse the range.
    pub(in crate::render) fn release_cell(
        &mut self,
        uploads: &mut StagedUploads,
        cell: u32,
    ) -> Result<(), UploadError> {
        for kind in [TableKind::Direct, TableKind::Bounce] {
            let (plan, offset) = match kind {
                TableKind::Direct => {
                    let plan = self.layout.plan_of_cell(cell);
                    (plan, plan.map(|plan| self.layout.base_byte_offset(plan)))
                }
                TableKind::Bounce => {
                    let plan = self.bounce_layout.plan_of_cell(cell);
                    (
                        plan,
                        plan.map(|plan| self.bounce_layout.base_byte_offset(plan)),
                    )
                }
            };
            let (Some(plan), Some(offset)) = (plan, offset) else {
                continue;
            };
            let table = self.table_mut(kind);
            if table
                .pool
                .as_mut()
                .and_then(|pool| pool.release(plan))
                .is_some()
            {
                uploads.write_buffer(&table.buffer, offset, &NO_BASE.to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// A new residency session starts with no cell resident.
    pub(in crate::render) fn clear_pools(&mut self, queue: &UploadQueue) {
        for kind in [TableKind::Direct, TableKind::Bounce] {
            let (bases, cells) = match kind {
                TableKind::Direct => (self.layout.base_byte_offset(0), self.layout.cell_count()),
                TableKind::Bounce => (
                    self.bounce_layout.base_byte_offset(0),
                    self.bounce_layout.cell_count(),
                ),
            };
            let table = self.table_mut(kind);
            let Some(pool) = table.pool.as_mut() else {
                continue;
            };
            pool.clear();
            if cells > 0 {
                let none: Vec<u8> = NO_BASE.to_le_bytes().repeat(cells);
                queue.direct_write_buffer(&table.buffer, bases, &none);
            }
        }
    }

    fn table(&self, kind: TableKind) -> &ChannelTable {
        match kind {
            TableKind::Direct => &self.direct,
            TableKind::Bounce => &self.bounce,
        }
    }

    fn table_mut(&mut self, kind: TableKind) -> &mut ChannelTable {
        match kind {
            TableKind::Direct => &mut self.direct,
            TableKind::Bounce => &mut self.bounce,
        }
    }

    #[cfg(test)]
    pub(in crate::render) fn layout(&self) -> &MoverChannelLayout {
        &self.layout
    }

    /// Each table's resident cells and the payload words their base names,
    /// read back from the GPU: what the compose reads for each cell.
    #[cfg(test)]
    pub(in crate::render) fn resident_cell_words(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> [std::collections::BTreeMap<u32, Vec<u32>>; 2] {
        let direct = read_table(device, queue, &self.direct.buffer);
        let bounce = read_table(device, queue, &self.bounce.buffer);
        let cells = |words: &[u32],
                     count: usize,
                     cell_of: &dyn Fn(usize) -> u32,
                     base_at: &dyn Fn(usize) -> u64,
                     lens: Vec<u32>| {
            (0..count)
                .filter_map(|plan| {
                    let base = words[(base_at(plan) / 4) as usize];
                    (base != NO_BASE).then(|| {
                        let base = base as usize;
                        (
                            cell_of(plan),
                            words[base..base + lens[plan] as usize].to_vec(),
                        )
                    })
                })
                .collect()
        };
        let direct_cells = self.layout.cell_numbers();
        let bounce_cells = self.bounce_layout.cell_numbers();
        [
            cells(
                &direct,
                self.layout.cell_count(),
                &|plan| direct_cells[plan],
                &|plan| self.layout.base_byte_offset(plan),
                self.layout.cell_payload_words(),
            ),
            cells(
                &bounce,
                self.bounce_layout.cell_count(),
                &|plan| bounce_cells[plan],
                &|plan| self.bounce_layout.base_byte_offset(plan),
                self.bounce_layout.cell_payload_words(),
            ),
        ]
    }
}

#[cfg(test)]
fn read_table(device: &wgpu::Device, queue: &wgpu::Queue, table: &wgpu::Buffer) -> Vec<u32> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mover channel table readback"),
        size: table.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(table, 0, &readback, 0, table.size());
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("table maps"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    let words = bytemuck::cast_slice(&slice.get_mapped_range().expect("mapped")).to_vec();
    readback.unmap();
    words
}

/// Write `write`'s payload into a whole table's image at `base`.
fn apply(words: &mut [u32], base: Option<u32>, write: &CellWrite) {
    let base = base.expect("a whole table places every cell at a fixed base") as usize;
    words[base..base + write.words.len()].copy_from_slice(&write.words);
}

/// Every bounce cell of a held payload, at its fixed base.
fn write_whole_bounce(
    layout: &BounceLayout,
    section: &MoverProbeChannelSection,
    words: &mut [u32],
) -> Result<(), MoverChannelError> {
    if layout.cell_count() == 0 {
        return Ok(());
    }
    let slices = section.cell_slices();
    for plan in 0..layout.cell_count() {
        let position = layout.position(plan);
        let payload = section
            .cell_payload(position, &slices[position], None)
            .map_err(|error| MoverChannelError(error.to_string()))?;
        let write = layout.cell_write(section, plan, payload)?;
        apply(words, layout.whole_base(plan), &write);
    }
    Ok(())
}

fn record_table(
    ledger: &mut ShAllocationLedger,
    kind: ShAllocationKind,
    words: &[u32],
    sources: [Option<u16>; 3],
) {
    let sources = source_ids(sources);
    let allocation = buffer_allocation(kind, bytemuck::cast_slice(words), TABLE_USAGE);
    let dummy = sources.is_empty();
    ledger.record_buffer(
        allocation,
        &sources,
        dummy,
        if dummy {
            ResidencyAllocationState::Dummy
        } else {
            ResidencyAllocationState::Data
        },
    );
}

/// Id 57's structure for a streamed level: everything but the depth block
/// and a resident bounce block, whose cells were written at load.
fn structure(section: &MoverProbeChannelSection) -> MoverProbeChannelSection {
    let bounce_streams = section.bounce_placement == MoverPayloadPlacement::Streamed;
    MoverProbeChannelSection {
        affinity_factor: section.affinity_factor,
        affinity_dims: section.affinity_dims,
        direct_placement: section.direct_placement,
        bounce_placement: section.bounce_placement,
        cells: section.cells.clone(),
        direct_offsets: section.direct_offsets.clone(),
        direct_records: section.direct_records.clone(),
        mover_offsets: section.mover_offsets.clone(),
        direct_movers: section.direct_movers.clone(),
        visibility: section.visibility.clone(),
        own_tiles: section.own_tiles.clone(),
        bounce_offsets: if bounce_streams {
            section.bounce_offsets.clone()
        } else {
            vec![0; section.cells.len() + 1]
        },
        bounce_entries: if bounce_streams {
            section.bounce_entries.clone()
        } else {
            Vec::new()
        },
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps: section.ramps.clone(),
    }
}
