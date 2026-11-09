// Mover probe channel depth (id 57): each probe a mover's pose changes gets a
// renderer-owned live tile after id 59's in the depth tile texture, holding
// the linear blend of that mover's closed-pose tiles D₀ and D₁ at its t′.
// This side is wgpu-free: it holds the closed-pose tiles on the CPU, decides
// which live tiles a frame rewrites and which probe references move, and
// blends the texels. `crate::render::mover_depth_tiles` stages the writes.
// See: context/lib/rendering_pipeline.md §4 (Direction-aware probe
// visibility, Mover state lighting)
//
// IP design rules (ip-372 §Design levers): mover state only blends moment
// sets. It never flags a probe occluded, zeroes a weight or excludes a probe;
// the sampler, its 0.03 floor and its bilinear tile tap are unchanged.

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;
use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_BORDERED_EDGE, PROBE_DEPTH_TILE_TEXEL_BYTES, PROBE_DEPTH_TILE_TEXELS,
    ProbeDepthTileTextureLayout, f16_bits_to_f32,
};
use postretro_render_cpu::sh_depth_tiles::ProbeDepthTileReferences;

const EDGE: usize = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
const TEXEL_BYTES: usize = PROBE_DEPTH_TILE_TEXEL_BYTES as usize;
const TILE_BYTES: usize = PROBE_DEPTH_TILE_TEXELS * TEXEL_BYTES;

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
    /// Each record's `D₀` then `D₁` texels, in `record_slots` order: the
    /// CPU-held closed-pose tiles, decoded once from id 57's f16 (exactly) so
    /// a frame's blend converts only its result.
    closed_pose: Vec<[f32; 2]>,
    /// Probes per mover slot (CSR over `slot_probes`), as indices into
    /// `probes`, ascending.
    slot_offsets: Vec<u32>,
    slot_probes: Vec<u32>,
    /// Set once the level's tile texture holds the live tiles.
    binding: Option<LiveTileBinding>,
    /// Whether probe `k` references its live tile now.
    pointed: Vec<bool>,
    dirty: Vec<bool>,
    dirty_list: Vec<u32>,
    reference_changes: Vec<ReferenceChange>,
    /// One run of live tiles on one tile row, tile after tile as texel
    /// bytes, then the run's texels row-major across the row as the texture
    /// copy reads them.
    run_tiles: Vec<u8>,
    run_bytes: Vec<u8>,
    /// Live tiles the last planned frame wrote, for tests.
    tiles_written: u32,
}

impl MoverDepthTiles {
    /// `section`'s depth records over the channel's mover slots (`mover_ids`,
    /// ascending; every record's mover is one of them by load validation).
    pub(in crate::render) fn new(
        section: Option<&MoverProbeChannelSection>,
        mover_ids: &[u32],
    ) -> Self {
        let Some(section) = section.filter(|section| !section.depth_records.is_empty()) else {
            return Self::default();
        };
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
        let mut probe_offsets = Vec::with_capacity(probes.len() + 1);
        let mut record_slots = Vec::with_capacity(records);
        let mut closed_pose = Vec::with_capacity(records * 2 * PROBE_DEPTH_TILE_TEXELS);
        let mut slot_lists = vec![Vec::new(); mover_ids.len()];
        probe_offsets.push(0u32);
        for (k, group) in per_probe.iter().enumerate() {
            for &record in group {
                let slot = slot_of(section.depth_records[record].mover_id);
                record_slots.push(slot);
                let [d0, d1] = section.depth_tile_pair(record);
                closed_pose.extend(
                    d0.iter()
                        .chain(d1)
                        .map(|&[mean, mean_sq]| [f16_to_f32(mean), f16_to_f32(mean_sq)]),
                );
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
        Self {
            probes,
            probe_offsets,
            record_slots,
            closed_pose,
            slot_offsets,
            slot_probes,
            binding: None,
            pointed: vec![false; probe_count],
            dirty: vec![false; probe_count],
            dirty_list: Vec::with_capacity(probe_count),
            reference_changes: Vec::with_capacity(probe_count),
            run_tiles: Vec::new(),
            run_bytes: Vec::new(),
            tiles_written: 0,
        }
    }

    /// Live tiles the level's tile texture needs: one per probe carrying
    /// mover depth, however many movers change it.
    pub(in crate::render) fn live_tile_count(&self) -> u32 {
        self.probes.len() as u32
    }

    /// Bytes of closed-pose tiles held on the CPU: the ledger's host row.
    pub(in crate::render) fn host_bytes(&self) -> u64 {
        (self.closed_pose.len() * size_of::<[f32; 2]>()) as u64
    }

    /// Bind the live tiles to the installed tile texture: they follow id
    /// 59's `first_live_tile` tiles in `layout`, and `references` (the load's
    /// table, before any rewrite) gives each probe's reference without mover
    /// state. A level whose SH is unusable never binds, and plans nothing.
    pub(in crate::render) fn bind(
        &mut self,
        first_live_tile: u32,
        layout: ProbeDepthTileTextureLayout,
        references: &ProbeDepthTileReferences,
    ) {
        if self.probes.is_empty() {
            return;
        }
        let run_bytes = layout.tiles_per_row() as usize * TILE_BYTES;
        self.run_tiles = Vec::with_capacity(run_bytes);
        self.run_bytes = Vec::with_capacity(run_bytes);
        self.binding = Some(LiveTileBinding {
            first_live_tile,
            tiles_per_row_shift: layout.tiles_per_row_shift,
            base_references: self
                .probes
                .iter()
                .map(|&probe| references.tile_of(probe))
                .collect(),
        });
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
                if self.pointed[k as usize] {
                    self.pointed[k as usize] = false;
                    self.reference_changes
                        .push((self.probes[k as usize], binding.base_references[k as usize]));
                }
                continue;
            }
            if run_first.is_none() {
                run_first = Some(tile);
            }
            blend_probe(
                &self.closed_pose,
                &self.record_slots,
                records,
                t,
                &mut self.run_tiles,
            );
            run_len += 1;
            if !self.pointed[k as usize] {
                self.pointed[k as usize] = true;
                self.reference_changes
                    .push((self.probes[k as usize], Some(tile as u16)));
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
            self.closed_pose.capacity(),
        ]
    }
}

/// Append one probe's live tile to `out` as texel bytes: per texel, each
/// stated mover's `(1 − t′)·D₀ + t′·D₁` (exact at both termini), and where
/// several movers change the probe, the pair whose blended `E[d]` is
/// smallest, the nearer occluder (ties keep the lower mover slot). The
/// choice depends on the movers' t′ alone, never on hand-over order, and is
/// always one mover's blended moment pair, so the tile stays a continuous
/// moment set.
fn blend_probe(
    closed_pose: &[[f32; 2]],
    record_slots: &[u32],
    records: std::ops::Range<usize>,
    t: &[f32],
    out: &mut Vec<u8>,
) {
    let terminus = |record: usize| {
        let start = record * 2 * PROBE_DEPTH_TILE_TEXELS;
        (
            &closed_pose[start..start + PROBE_DEPTH_TILE_TEXELS],
            &closed_pose[start + PROBE_DEPTH_TILE_TEXELS..start + 2 * PROBE_DEPTH_TILE_TEXELS],
        )
    };
    let mut stated = records
        .clone()
        .filter(|&record| t[record_slots[record] as usize] >= 0.0);
    let first = stated.next().expect("a written probe has a stated mover");
    let at = out.len();
    out.resize(at + TILE_BYTES, 0);
    let (tile, _) = out[at..].as_chunks_mut::<TEXEL_BYTES>();
    let Some(second) = stated.next() else {
        // One mover: blend straight into bytes; a terminus copies its tile.
        let t = t[record_slots[first] as usize];
        let (d0, d1) = terminus(first);
        if t <= 0.0 || t >= 1.0 {
            let source = if t <= 0.0 { d0 } else { d1 };
            for (texel, &moments) in tile.iter_mut().zip(source) {
                *texel = texel_bytes(moments);
            }
        } else {
            let s = 1.0 - t;
            for ((texel, a), b) in tile.iter_mut().zip(d0).zip(d1) {
                *texel = texel_bytes([s * a[0] + t * b[0], s * a[1] + t * b[1]]);
            }
        }
        return;
    };
    let mut nearest = [[f32::INFINITY; 2]; PROBE_DEPTH_TILE_TEXELS];
    for record in [first, second].into_iter().chain(stated) {
        let t = t[record_slots[record] as usize];
        let (d0, d1) = terminus(record);
        for ((texel, a), b) in nearest.iter_mut().zip(d0).zip(d1) {
            let mean = lerp(a[0], b[0], t);
            if mean < texel[0] {
                *texel = [mean, lerp(a[1], b[1], t)];
            }
        }
    }
    for (texel, &moments) in tile.iter_mut().zip(&nearest) {
        *texel = texel_bytes(moments);
    }
}

/// One `Rg16Float` texel's little-endian bytes.
fn texel_bytes([mean, mean_sq]: [f32; 2]) -> [u8; TEXEL_BYTES] {
    (u32::from(f32_to_f16(mean)) | (u32::from(f32_to_f16(mean_sq)) << 16)).to_le_bytes()
}

/// Exact at both termini, whatever the moments hold.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        (1.0 - t) * a + t * b
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

/// Binary16 bits → f32, exact; normal values by bit shuffle.
fn f16_to_f32(bits: u16) -> f32 {
    let exponent = u32::from((bits >> 10) & 0x1f);
    if exponent == 0 || exponent == 0x1f {
        return f16_bits_to_f32(bits);
    }
    f32::from_bits(
        (u32::from(bits & 0x8000) << 16)
            | ((exponent + 112) << 23)
            | (u32::from(bits & 0x3ff) << 13),
    )
}

/// f32 → binary16 bits, round to nearest even; the normal range by integer
/// rounding, everything else through the shared converter.
fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let magnitude = bits & 0x7fff_ffff;
    // [2^-14, 65520): normal binary16 results, no overflow to infinity.
    if (0x3880_0000..0x477f_f000).contains(&magnitude) {
        let rounded = magnitude.wrapping_add(0x0fff + ((magnitude >> 13) & 1)) >> 13;
        return (sign | rounded.wrapping_sub(112 << 10)) as u16;
    }
    f32_to_f16_bits(value)
}

#[cfg(test)]
#[path = "depth_tests.rs"]
mod tests;
