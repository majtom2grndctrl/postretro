// Mover probe channel depth (id 57): each probe a mover's pose changes gets a
// renderer-owned live tile after id 59's in the depth tile texture, holding
// that mover's closed-pose tiles blended by its t′ between the two poses
// around it that store a tile or read the probe's base moment set (its id-59
// tile or averaged id-34 moments); an unmarked mid pose is blended across.
// The format's depth rule says how (owner, 2026-11-11): an interval with a
// mid pose at either end blends its two poses' mean and variance, which
// follows a surface the mover translates; one between the two termini runs
// its raw moments through the base, reaching it at the interval's midpoint,
// as a two-pose mover always has. A pose with no tile is one the mover
// leaves unchanged or encloses: it is the base, so there the live tile
// holds exactly what the probe reads with no section. Several movers at one
// probe add their blended differences from that base, per moment, clamped
// to a valid moment set (owner, 2026-11-09).
// This side is wgpu-free: it holds the closed-pose tiles on the CPU, decides
// which live tiles a frame rewrites and which probe references move, and
// blends the texels. `crate::render::mover_depth_tiles` stages the writes.
// See: context/lib/rendering_pipeline.md §4 (Direction-aware probe
// visibility, Mover state lighting)
//
// IP design rules (ip-372 §Design levers): mover state only blends moment
// sets, and several movers combine by adding differences, never by selecting
// one. It never flags a probe occluded, zeroes a weight or excludes a probe;
// the sampler, its 0.03 floor and its bilinear tile tap are unchanged.

#[cfg(test)]
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    MoverProbeChannelSection, PoseMark, is_terminus,
};
#[cfg(test)]
use postretro_level_format::probe_depth_tiles::f16_bits_to_f32;
use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_BORDERED_EDGE, PROBE_DEPTH_TILE_TEXEL_BYTES, PROBE_DEPTH_TILE_TEXELS,
    ProbeDepthTileTextureLayout, ProbeDepthTilesSection,
};
use postretro_level_format::sh_volume::OctahedralShProbe;
use postretro_render_cpu::sh_depth_tiles::ProbeDepthTileReferences;

#[cfg(test)]
use super::depth_blend::f32_to_f16;
use super::depth_blend::{ACROSS, TILE_MOMENTS, UNBOUND_BASE, blend_probe, f16_to_f32, segment};
use super::knots::MoverKnots;
use super::layout::{MoverChannelError, first_non_finite};

const EDGE: usize = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
const TEXEL_BYTES: usize = PROBE_DEPTH_TILE_TEXEL_BYTES as usize;
const TILE_BYTES: usize = PROBE_DEPTH_TILE_TEXELS * TEXEL_BYTES;

/// What each probe reads with no mover state: the load's id-59 tiles and
/// its id-34 probe moments. A record pose that stores no tile reads this.
#[derive(Debug, Clone, Copy)]
pub(in crate::render) struct DepthBases<'a> {
    pub(in crate::render) id59: Option<&'a ProbeDepthTilesSection>,
    /// The level's id-34 probes, whole-loaded or the streamed base metadata.
    pub(in crate::render) probes: &'a [OctahedralShProbe],
}

/// One probe reference the frame moves: onto the probe's live tile while
/// some mover changing it has state, back to its id-59 tile or averaged
/// moments (`None`) once none does.
pub(in crate::render) type ReferenceChange = (u32, Option<u16>);

/// Where the live tiles sit in the installed tile texture.
#[derive(Debug, Clone)]
struct LiveTileBinding {
    /// Live tile of probe `k` is `first_live_tile + k`: id 59's tile count.
    first_live_tile: u32,
    tiles_per_row_shift: u32,
    /// Each probe's reference without mover state: its id-59 tile, or none.
    base_references: Vec<Option<u16>>,
}

/// Id 57's depth block on the CPU and the live tiles' per-frame plan. Sized
/// once per level; a frame allocates nothing.
#[derive(Debug, Clone, Default)]
pub(in crate::render) struct MoverDepthTiles {
    /// Distinct probes carrying mover depth, in live-tile order: probe `k`
    /// owns live tile `first_live_tile + k`.
    probes: Vec<u32>,
    /// Records per probe (CSR over `record_slots`), ascending by mover slot.
    probe_offsets: Vec<u32>,
    /// Each record's mover slot, in probe order.
    record_slots: Vec<u32>,
    /// Each probe's base tile as an offset into `tiles` once bound,
    /// [`UNBOUND_BASE`] before: a blend between two termini passes through
    /// it, and a pose that reads the base reads it.
    base_tiles: Vec<u32>,
    /// Each record's poses (CSR over `record_poses`, in `record_slots`
    /// order), each an offset into `tiles`: its stored tile, or once
    /// bound its probe's base tile where it reads the base
    /// ([`UNBOUND_BASE`] before), or [`ACROSS`] where a mid pose is
    /// blended across.
    record_pose_offsets: Vec<u32>,
    record_poses: Vec<u32>,
    /// Each mover slot's knots through its ramp.
    knots: MoverKnots,
    /// The CPU-held tiles, decoded once from f16 (exactly) so a frame's blend
    /// converts only its result: every stored closed-pose tile, every pose's,
    /// then, from [`Self::bind`], one base tile per probe. Each tile spans
    /// [`TILE_MOMENTS`] values, its texels' means then their second moments,
    /// so the blend runs each moment across full vector lanes.
    tiles: Vec<f32>,
    /// Probes per mover slot (CSR over `slot_probes`), as indices into
    /// `probes`, ascending.
    slot_offsets: Vec<u32>,
    slot_probes: Vec<u32>,
    /// Set once the level's tile texture holds the live tiles.
    binding: Option<LiveTileBinding>,
    /// Whether probe `k` references its live tile now.
    pointed: Vec<bool>,
    /// Whether probe `k`'s reference is unknown: a move planned for it was
    /// abandoned, and may or may not have landed. Its next plan emits its
    /// move whatever `pointed` says.
    unsure: Vec<bool>,
    dirty: Vec<bool>,
    dirty_list: Vec<u32>,
    reference_changes: Vec<ReferenceChange>,
    /// The live-tile index `k` of each entry in `reference_changes`, so a
    /// frame whose reference moves fail can mark those probes `unsure`.
    changed_live: Vec<u32>,
    /// One run of live tiles on one tile row, tile after tile as texel
    /// bytes, then the run's texels row-major across the row as the texture
    /// copy reads them. Sized once in tiles: a run holds at most a tile
    /// row's worth, and never more than the level's live tiles.
    run_tiles: Vec<u8>,
    run_bytes: Vec<u8>,
    /// Live tiles the last planned frame wrote, for tests.
    tiles_written: u32,
}

impl MoverDepthTiles {
    /// `section`'s depth records over the channel's mover slots (`mover_ids`,
    /// ascending; every record's mover is one of them by load validation).
    /// Every closed-pose tile must hold finite moments, checked as bounce
    /// tiles are: a non-finite one refuses the install.
    pub(in crate::render) fn new(
        section: Option<&MoverProbeChannelSection>,
        mover_ids: &[u32],
    ) -> Result<Self, MoverChannelError> {
        let Some(section) = section.filter(|section| !section.depth_records.is_empty()) else {
            return Ok(Self::default());
        };
        if let Some(bad) = first_non_finite(section.depth_tiles.iter().flatten().copied()) {
            return Err(MoverChannelError(format!(
                "a closed-pose depth tile holds a non-finite moment ({bad:#06x})"
            )));
        }
        let slot_of = |mover: u32| {
            mover_ids
                .binary_search(&mover)
                .expect("a depth record names a channel mover") as u32
        };
        // Live tiles go in order of first appearance in the wire's (mover,
        // probe) order: each mover's probes sit together, the ones it shares
        // with a lower mover with that mover's, so a travelling mover's tiles
        // mostly form one run per tile row and reach the texture in few
        // copies. Each probe's records keep mover-slot order.
        let mut live_of = std::collections::HashMap::new();
        let mut probes = Vec::new();
        let mut per_probe: Vec<Vec<usize>> = Vec::new();
        for (record, depth) in section.depth_records.iter().enumerate() {
            let k = *live_of.entry(depth.probe_index).or_insert_with(|| {
                probes.push(depth.probe_index);
                per_probe.push(Vec::new());
                probes.len() - 1
            });
            per_probe[k].push(record);
        }
        let records = section.depth_records.len();
        let views: Vec<_> = section.depth_record_views().collect();
        let stored_texels = section.depth_tiles.len();
        let mut probe_offsets = Vec::with_capacity(probes.len() + 1);
        let mut record_slots = Vec::with_capacity(records);
        let mut record_pose_offsets = Vec::with_capacity(records + 1);
        record_pose_offsets.push(0u32);
        let mut record_poses = Vec::new();
        let mut tiles = Vec::with_capacity(2 * stored_texels);
        let mut slot_lists = vec![Vec::new(); mover_ids.len()];
        probe_offsets.push(0u32);
        for (k, group) in per_probe.iter().enumerate() {
            for &record in group {
                let slot = slot_of(section.depth_records[record].mover_id);
                record_slots.push(slot);
                let view = &views[record];
                let poses = view.record.pose_count();
                for (pose, &mark) in view.record.marks.iter().enumerate() {
                    record_poses.push(match (mark, view.pose_tile(pose)) {
                        (PoseMark::Stored, Some(tile)) => {
                            let at = tiles.len() as u32;
                            push_tile(&mut tiles, tile);
                            at
                        }
                        (PoseMark::Unmarked, _) if !is_terminus(pose, poses) => ACROSS,
                        _ => UNBOUND_BASE,
                    });
                }
                record_pose_offsets.push(record_poses.len() as u32);
                slot_lists[slot as usize].push(k as u32);
            }
            probe_offsets.push(record_slots.len() as u32);
        }
        let mut slot_offsets = vec![0u32];
        let mut slot_probes = Vec::new();
        for mut list in slot_lists {
            list.sort_unstable();
            slot_probes.extend(list);
            slot_offsets.push(slot_probes.len() as u32);
        }
        let probe_count = probes.len();
        Ok(Self {
            probes,
            probe_offsets,
            record_slots,
            record_pose_offsets,
            record_poses,
            knots: MoverKnots::new(Some(section), mover_ids)?,
            base_tiles: vec![UNBOUND_BASE; probe_count],
            tiles,
            slot_offsets,
            slot_probes,
            binding: None,
            pointed: vec![false; probe_count],
            unsure: vec![false; probe_count],
            dirty: vec![false; probe_count],
            dirty_list: Vec::with_capacity(probe_count),
            reference_changes: Vec::with_capacity(probe_count),
            changed_live: Vec::with_capacity(probe_count),
            run_tiles: Vec::new(),
            run_bytes: Vec::new(),
            tiles_written: 0,
        })
    }

    /// Live tiles the level's tile texture needs: one per probe carrying
    /// mover depth, however many movers change it.
    pub(in crate::render) fn live_tile_count(&self) -> u32 {
        self.probes.len() as u32
    }

    /// Bytes of tiles held on the CPU, the ledger's host row: the stored
    /// closed-pose tiles, and once bound one base tile per probe.
    pub(in crate::render) fn host_bytes(&self) -> u64 {
        (self.tiles.len() * size_of::<f32>()) as u64
    }

    /// Bind the live tiles to the installed tile texture: they follow id
    /// 59's `first_live_tile` tiles in `layout`, and `references` (the load's
    /// table, before any rewrite) gives each probe's reference without mover
    /// state. Every probe carrying mover depth gets one base tile from
    /// `bases`, which termini-to-termini blends pass and base-reading poses
    /// read: the id-59 tile its reference names, else its averaged id-34
    /// moments in every texel, exactly what the probe reads with no section.
    /// A level whose SH is unusable never binds, and plans nothing. An
    /// untiled probe outside `bases.probes` has no base to blend through and
    /// refuses the install, never a silent zero moment set.
    pub(in crate::render) fn bind(
        &mut self,
        first_live_tile: u32,
        layout: ProbeDepthTileTextureLayout,
        references: &ProbeDepthTileReferences,
        bases: DepthBases<'_>,
    ) -> Result<(), MoverChannelError> {
        if self.probes.is_empty() {
            return Ok(());
        }
        // Exactly the base tiles, so the host ledger row is what is held.
        self.tiles.reserve_exact(self.probes.len() * TILE_MOMENTS);
        for (k, &probe) in self.probes.iter().enumerate() {
            let records = self.probe_offsets[k] as usize..self.probe_offsets[k + 1] as usize;
            let at = self.tiles.len() as u32;
            self.base_tiles[k] = at;
            match (references.tile_of(probe), bases.id59) {
                (Some(tile), Some(id59)) => {
                    push_tile(&mut self.tiles, id59.tile(usize::from(tile)))
                }
                _ => {
                    let Some(base) = bases.probes.get(probe as usize) else {
                        return Err(MoverChannelError(format!(
                            "a closed-pose depth record names probe {probe}, outside the level's {} id-34 probe(s), with no id-59 tile to blend through",
                            bases.probes.len()
                        )));
                    };
                    for moment in [base.mean_distance, base.mean_sq_distance] {
                        self.tiles.extend(std::iter::repeat_n(
                            f16_to_f32(moment),
                            PROBE_DEPTH_TILE_TEXELS,
                        ));
                    }
                }
            }
            let poses = self.record_pose_offsets[records.start] as usize
                ..self.record_pose_offsets[records.end] as usize;
            for pose in &mut self.record_poses[poses] {
                if *pose == UNBOUND_BASE {
                    *pose = at;
                }
            }
        }
        let run_tiles = (layout.tiles_per_row() as usize).min(self.probes.len());
        self.run_tiles = Vec::with_capacity(run_tiles * TILE_BYTES);
        self.run_bytes = Vec::with_capacity(run_tiles * TILE_BYTES);
        self.binding = Some(LiveTileBinding {
            first_live_tile,
            tiles_per_row_shift: layout.tiles_per_row_shift,
            base_references: self
                .probes
                .iter()
                .map(|&probe| references.tile_of(probe))
                .collect(),
        });
        Ok(())
    }

    /// Live tiles the last planned frame wrote.
    #[cfg(test)]
    pub(in crate::render) fn tiles_written(&self) -> u32 {
        self.tiles_written
    }

    /// Plan one frame from each slot's t′ (`t`, negative for no state) and
    /// the slots whose t′ moved since the last write (`changed`). Every live
    /// tile a changed mover covers is rewritten from every mover changing
    /// that probe, so a tile shared with a held mover reads its held t′.
    /// Runs of live tiles on one tile row reach `write_run(first tile, tile
    /// count, bytes)` before the reference changes return; the caller stages
    /// both on one upload batch. Allocates nothing.
    pub(in crate::render) fn plan(
        &mut self,
        t: &[f32],
        changed: &[u32],
        mut write_run: impl FnMut(u32, u32, &[u8]),
    ) -> &[ReferenceChange] {
        self.reference_changes.clear();
        self.changed_live.clear();
        self.tiles_written = 0;
        let Some(binding) = self.binding.as_ref() else {
            return &self.reference_changes;
        };
        for &slot in changed {
            let slot = slot as usize;
            let Some(range) = self.slot_offsets.get(slot..slot + 2) else {
                continue;
            };
            for &probe in &self.slot_probes[range[0] as usize..range[1] as usize] {
                if !self.dirty[probe as usize] {
                    self.dirty[probe as usize] = true;
                    self.dirty_list.push(probe);
                }
            }
        }
        self.dirty_list.sort_unstable();

        let mut run_first: Option<u32> = None;
        let mut run_len = 0u32;
        for at in 0..self.dirty_list.len() {
            let k = self.dirty_list[at];
            self.dirty[k as usize] = false;
            let tile = binding.first_live_tile + k;
            let records = self.probe_offsets[k as usize] as usize
                ..self.probe_offsets[k as usize + 1] as usize;
            let stated = records
                .clone()
                .any(|record| t[self.record_slots[record] as usize] >= 0.0);
            let continues = run_first.is_some_and(|first| {
                first + run_len == tile
                    && first >> binding.tiles_per_row_shift == tile >> binding.tiles_per_row_shift
            });
            if !stated || !continues {
                if let Some(first) = run_first.take() {
                    flush_run(
                        &self.run_tiles,
                        &mut self.run_bytes,
                        first,
                        run_len,
                        &mut write_run,
                    );
                    self.tiles_written += run_len;
                }
                self.run_tiles.clear();
                run_len = 0;
            }
            if !stated {
                // No mover changing this probe has state: its id-59 tile or
                // averaged moments again, bit for bit.
                if self.pointed[k as usize] || self.unsure[k as usize] {
                    self.pointed[k as usize] = false;
                    self.unsure[k as usize] = false;
                    self.reference_changes
                        .push((self.probes[k as usize], binding.base_references[k as usize]));
                    self.changed_live.push(k);
                }
                continue;
            }
            if run_first.is_none() {
                run_first = Some(tile);
            }
            let base_at = self.base_tiles[k as usize] as usize;
            let base = &self.tiles[base_at..base_at + TILE_MOMENTS];
            let (tiles, poses, offsets) =
                (&self.tiles, &self.record_poses, &self.record_pose_offsets);
            let (slots, knots) = (&self.record_slots, &self.knots);
            let blends = records.filter_map(|record| {
                let slot = slots[record] as usize;
                (t[slot] >= 0.0).then(|| {
                    let poses = &poses[offsets[record] as usize..offsets[record + 1] as usize];
                    segment(tiles, poses, knots.slot(slot), base, t[slot])
                })
            });
            blend_probe(base, blends, &mut self.run_tiles);
            run_len += 1;
            if !self.pointed[k as usize] || self.unsure[k as usize] {
                self.pointed[k as usize] = true;
                self.unsure[k as usize] = false;
                self.reference_changes
                    .push((self.probes[k as usize], Some(tile as u16)));
                self.changed_live.push(k);
            }
        }
        if let Some(first) = run_first {
            flush_run(
                &self.run_tiles,
                &mut self.run_bytes,
                first,
                run_len,
                &mut write_run,
            );
            self.tiles_written += run_len;
        }
        self.run_tiles.clear();
        self.dirty_list.clear();
        &self.reference_changes
    }

    /// The last plan's reference moves did not all land, and any of them
    /// may have: mark each probe they name unsure, so the next plan that
    /// reaches it emits its move again whatever it planned last, landed or
    /// not, including a move back to its base. The caller replans those
    /// probes next frame (`MoverTBlock::forget_depth_changes`); their tile
    /// writes are rewritten then too, so nothing else needs undoing.
    pub(in crate::render) fn abandon_plan(&mut self) {
        for &k in &self.changed_live {
            self.unsure[k as usize] = true;
        }
        self.changed_live.clear();
        self.reference_changes.clear();
    }

    #[cfg(test)]
    pub(in crate::render) fn probes(&self) -> &[u32] {
        &self.probes
    }

    #[cfg(test)]
    pub(in crate::render) fn capacities(&self) -> [usize; 5] {
        [
            self.dirty_list.capacity(),
            self.reference_changes.capacity(),
            self.run_tiles.capacity(),
            self.run_bytes.capacity(),
            self.tiles.capacity(),
        ]
    }
}

/// Append one f16 tile to `tiles` as the blend holds it: its texels'
/// means, then their second moments, decoded exactly.
fn push_tile(tiles: &mut Vec<f32>, tile: &[[u16; 2]]) {
    for moment in 0..2 {
        tiles.extend(tile.iter().map(|texel| f16_to_f32(texel[moment])));
    }
}

/// Lay a run's tiles out as the texture copy reads them, row-major across
/// the run, and hand it to `write_run`.
fn flush_run(
    run_tiles: &[u8],
    run_bytes: &mut Vec<u8>,
    first: u32,
    count: u32,
    write_run: &mut impl FnMut(u32, u32, &[u8]),
) {
    const ROW_BYTES: usize = EDGE * TEXEL_BYTES;
    let count = count as usize;
    run_bytes.clear();
    run_bytes.resize(count * TILE_BYTES, 0);
    for (tile, source) in run_tiles.as_chunks::<TILE_BYTES>().0.iter().enumerate() {
        for (row, segment) in source.as_chunks::<ROW_BYTES>().0.iter().enumerate() {
            let at = (row * count + tile) * ROW_BYTES;
            run_bytes[at..at + ROW_BYTES].copy_from_slice(segment);
        }
    }
    write_run(first, count as u32, run_bytes);
}

#[cfg(test)]
#[path = "depth_tests.rs"]
mod tests;
