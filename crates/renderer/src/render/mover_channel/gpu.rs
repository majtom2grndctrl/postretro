// Mover probe channel GPU table: one read-only storage buffer per level that
// both direct-SH compose passes bind at binding 25, its streamed cell writes,
// and the per-frame t′ block inside it.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::mover_probe_channel::{
    MoverCellSlices, MoverChannelBlock, MoverProbeChannelSection, MoverStreamedRow,
};

use super::layout::{
    CellWrite, MoverChannelError, MoverChannelInputs, MoverChannelLayout, MoverChannelRows, TermRow,
};
use super::state::MoverTBlock;
use crate::render::MoverLightState;
use crate::render::gpu_create::GpuCreate;
use crate::render::residency::{ResidencyAllocationState, source_ids};
use crate::render::sh_allocation::{ShAllocationKind, buffer_allocation};
use crate::render::sh_residency::ShAllocationLedger;
use crate::render::uploads::{StagedUploads, UploadQueue};

/// The direct half of id 57 a streamed level keeps for cells whose payload
/// or terms arrive with later clusters.
#[derive(Debug)]
struct PendingCells {
    /// Id 57 without its bounce and depth blocks: indexes, records, masks,
    /// mover entries, ramps and, when resident, the direct payload.
    section: MoverProbeChannelSection,
    slices: Vec<MoverCellSlices>,
    written: Vec<bool>,
}

/// One streamed sparse row of id 41 or id 45: its affinity cell, each
/// entry's first f16 half in `tiles`, and the tiles.
#[derive(Clone, Copy, Debug)]
pub(in crate::render) struct ClusterSparseRow<'a> {
    pub(in crate::render) cell: u32,
    pub(in crate::render) entry_offsets: &'a [u32],
    pub(in crate::render) tiles: &'a [u16],
}

/// What one cluster brings that the channel reads: its first-written id-41
/// and id-45 rows and its id-57 rows.
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

pub(in crate::render) struct MoverChannelResources {
    buffer: wgpu::Buffer,
    layout: MoverChannelLayout,
    pending: Option<PendingCells>,
    t: MoverTBlock,
}

impl MoverChannelResources {
    /// Lay out the table and write every cell the load already holds:
    /// a whole load writes them all from `whole_rows` (cell, is-id-45 →
    /// that family's row); a streamed one only cells that borrow no
    /// streamed term, the rest arriving with their clusters.
    pub(in crate::render) fn new<'a>(
        device: &wgpu::Device,
        inputs: &MoverChannelInputs<'_>,
        streaming: bool,
        whole_rows: impl FnMut(u32, bool) -> Option<TermRow<'a>>,
        ledger: &mut ShAllocationLedger,
    ) -> Result<Self, MoverChannelError> {
        let layout = MoverChannelLayout::build(inputs)?;
        let mut words = layout.initial_words().to_vec();
        let mut pending = None;
        if let Some(section) = inputs.section {
            if streaming {
                // Keep only the direct half; cells that borrow a streamed
                // term or payload write as their clusters arrive.
                let half = direct_half(section);
                let slices = half.cell_slices();
                let writes = layout.held_writes(&half, &slices, |_, _| None)?;
                let mut written = vec![false; layout.cell_count()];
                for write in &writes {
                    apply(&mut words, write);
                    written[write.plan] = true;
                }
                if written.iter().any(|written| !written) {
                    pending = Some(PendingCells {
                        section: half,
                        slices,
                        written,
                    });
                }
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
                    apply(&mut words, write);
                }
            }
            if layout.ungated_records() > 0 {
                log::warn!(
                    "[Renderer] MoverProbeChannel: {} gated term record(s) compose ungated: their DirectShDeltaVolumes (id 41) or AnimatedDirectShDeltaVolumes (id 45) is absent at load",
                    layout.ungated_records()
                );
            }
            log::info!(
                "[Renderer] MoverProbeChannel: {} direct record(s) over {} cell(s) for {} mover(s), table {} B",
                layout.record_count(),
                layout.cell_count(),
                layout.mover_ids().len(),
                words.len() * 4,
            );
        }
        let contents: &[u8] = bytemuck::cast_slice(&words);
        let allocation = buffer_allocation(
            ShAllocationKind::MoverChannelTable,
            contents,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let present = inputs.section.is_some();
        ledger.record_buffer(
            allocation,
            &source_ids([
                present.then_some(57),
                (present && inputs.static_terms.is_some()).then_some(41),
                inputs.animated_terms.is_some().then_some(45),
            ]),
            !present,
            if present {
                ResidencyAllocationState::Data
            } else {
                ResidencyAllocationState::Dummy
            },
        );
        let buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mover Probe Channel Table"),
            contents,
            usage: allocation.usage,
        });
        Ok(Self {
            buffer,
            t: MoverTBlock::new(layout.mover_ids()),
            layout,
            pending,
        })
    }

    /// No section and no Pass B: the table header alone.
    pub(in crate::render) fn empty(device: &wgpu::Device) -> Self {
        let layout = MoverChannelLayout::empty(&[]);
        let buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mover Probe Channel Table (empty)"),
            contents: bytemuck::cast_slice(layout.initial_words()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            buffer,
            t: MoverTBlock::new(&[]),
            layout,
            pending: None,
        }
    }

    pub(in crate::render) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(in crate::render) fn rows(&self) -> &MoverChannelRows {
        self.layout.rows()
    }

    /// Take this frame's mover states; idempotent within a frame.
    pub(in crate::render) fn set_states(&mut self, states: &[MoverLightState]) {
        self.t.set_states(states);
    }

    /// Write the t′ block when it moved. Staged writes precede the frame's
    /// first submission, so compose recorded this frame reads this frame's t′.
    pub(in crate::render) fn upload_t(&mut self, queue: &UploadQueue) {
        let offset = self.layout.t_byte_offset();
        if let Some(bytes) = self.t.take_upload() {
            queue.write_buffer(&self.buffer, offset, bytes);
        }
    }

    /// Slots whose t′ moved since the streamed planner last looked.
    pub(in crate::render) fn observe_t_changes(&mut self) -> &[u32] {
        self.t.observe_changes()
    }

    /// The legacy compose's mover-state trigger.
    pub(in crate::render) fn t_changed_since_composed(&self) -> bool {
        self.t.changed_since_composed()
    }

    /// The legacy compose encoded this frame's t′: commit its trigger.
    pub(in crate::render) fn commit_composed_t(&mut self) {
        self.t.commit_composed();
    }

    /// Whether the table still waits on a cell's payload or terms.
    pub(in crate::render) fn has_pending_cells(&self) -> bool {
        self.pending.is_some()
    }

    /// The writes one streamed cluster completes: each unwritten cell whose
    /// streamed payload or delta rows the cluster carries, checked as the
    /// loader checks a held payload. A cell the cluster names but cannot
    /// complete is refused, so no row composes without its gate.
    pub(in crate::render) fn stage_cluster(
        &self,
        rows: &ClusterTermRows<'_>,
    ) -> Result<Vec<CellWrite>, MoverChannelError> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(Vec::new());
        };
        let mut cells: Vec<u32> = rows
            .static_rows
            .iter()
            .map(|row| row.cell)
            .chain(rows.animated_rows.iter().map(|row| row.cell))
            .chain(rows.channel_rows.iter().map(|row| row.cell))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        let streamed = !pending.section.payload_held(MoverChannelBlock::Direct);
        let mut writes = Vec::new();
        for cell in cells {
            let Some(plan) = self.layout.plan_of_cell(cell) else {
                continue;
            };
            if pending.written[plan] {
                continue;
            }
            let (needs_static, needs_animated) = self.layout.plan_needs(plan);
            let channel_row = rows
                .channel_rows
                .iter()
                .find(|row| row.cell == cell)
                .copied();
            let (static_valid, animated_valid) = self.layout.plan_valid_masks(plan);
            let static_row = find_row(&rows.static_rows, cell, static_valid);
            let animated_row = find_row(&rows.animated_rows, cell, animated_valid);
            if (streamed && channel_row.is_none())
                || (needs_static && static_row.is_none())
                || (needs_animated && animated_row.is_none())
            {
                return Err(MoverChannelError(format!(
                    "a cluster brings part of affinity cell {cell}'s gated terms but not all of them"
                )));
            }
            let payload =
                self.layout
                    .plan_payload(&pending.section, &pending.slices, plan, channel_row)?;
            writes.push(self.layout.cell_write(
                &pending.section,
                plan,
                payload,
                static_row,
                animated_row,
            )?);
        }
        Ok(writes)
    }

    /// Stage `writes` into a cluster's upload batch.
    pub(in crate::render) fn stage_writes(
        &self,
        uploads: &mut StagedUploads,
        writes: &[CellWrite],
    ) -> Result<(), crate::render::uploads::UploadError> {
        for write in writes {
            uploads.write_buffer_f16(&self.buffer, write.tile_byte_offset, &write.tile_halves)?;
            uploads.write_buffer(&self.buffer, write.q_byte_offset, &write.q_bytes)?;
        }
        Ok(())
    }

    /// The cluster's writes were submitted: their cells never rewrite.
    pub(in crate::render) fn mark_written(&mut self, writes: &[CellWrite]) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        for write in writes {
            pending.written[write.plan] = true;
        }
        if pending.written.iter().all(|written| *written) {
            self.pending = None;
        }
    }

    #[cfg(test)]
    pub(in crate::render) fn layout(&self) -> &MoverChannelLayout {
        &self.layout
    }
}

/// Write `write` into the table image.
fn apply(words: &mut [u32], write: &CellWrite) {
    let tile_word = (write.tile_byte_offset / 4) as usize;
    for (at, pair) in write.tile_halves.chunks(2).enumerate() {
        let low = u32::from(pair[0]);
        let high = u32::from(pair.get(1).copied().unwrap_or(0));
        words[tile_word + at] = low | (high << 16);
    }
    let q_word = (write.q_byte_offset / 4) as usize;
    for (at, quad) in write.q_bytes.chunks(4).enumerate() {
        let mut bytes = [0u8; 4];
        bytes[..quad.len()].copy_from_slice(quad);
        words[q_word + at] = u32::from_le_bytes(bytes);
    }
}

/// Id 57 without its bounce and depth blocks.
fn direct_half(section: &MoverProbeChannelSection) -> MoverProbeChannelSection {
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
        bounce_offsets: vec![0; section.cells.len() + 1],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps: section.ramps.clone(),
    }
}
