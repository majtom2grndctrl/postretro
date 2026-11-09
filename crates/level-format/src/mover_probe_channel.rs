// MoverProbeChannel PRL section (ID 57): what each analysed mover changes in
// the probe data at its two termini (per-light direct visibility, absolute
// static bounce, closed-pose depth tiles) and each mover's author light ramp.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

mod assemble;
mod error;
mod streamed;
mod views;
mod wire;

pub use assemble::{BounceEntryParts, DepthRecordParts, DirectRecordParts, MoverProbeChannelParts};
pub use error::MoverProbeChannelError;
pub use streamed::{
    MOVER_STREAMED_BLOCK_HEADER_BYTES, MOVER_STREAMED_ROW_BYTES, MoverCellSlices, MoverStreamedRow,
    MoverStreamedRowLens, parse_streamed_rows,
};
pub use views::{
    BounceEntryView, DirectCellView, DirectRecordView, MoverCellPayload, MoverChannelBytes,
    MoverDirectCounts, MoverHalves, MoverVisibilityView,
};

use crate::delta_sh_volumes::DEFAULT_DELTA_PROBE_F16_STRIDE;
use crate::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

/// Tiles a level's probe depth tile texture holds: id 59's `id59_tiles`,
/// then one live tile per probe `channel` carries mover depth at. The
/// compiler reserves, the loader bounds and the renderer sizes against this
/// one count; closed-pose tiles stay on the CPU and never count.
pub fn depth_tile_texture_count(
    id59_tiles: usize,
    channel: Option<&MoverProbeChannelSection>,
) -> usize {
    id59_tiles + channel.map_or(0, MoverProbeChannelSection::live_tile_count)
}

/// Section-internal epoch, the first byte of the payload. Exact match.
/// Epoch 2: sparse cell list, per-(cell, light) term records with a probe
/// mask and one own tile, per-(record, mover) visibility. Epoch 3: a
/// placement byte for the direct payload beside the bounce payload's.
pub const MOVER_PROBE_CHANNEL_EPOCH: u8 = 3;

/// Term-record flag bit 0: the record carries its light's own tile per
/// masked probe, because no per-probe id-41 or id-45 entry holds the light's
/// term at the cell. Every other flag bit is reserved and refused.
pub const MOVER_RECORD_FLAG_OWN_TILE: u8 = 1 << 0;

/// Visibility bytes per (mover entry, masked probe): `q₀` then `q₁`, each
/// `byte / 255`.
pub const MOVER_VISIBILITY_BYTES_PER_PROBE: usize = 2;

/// One compact RGB f16 octahedral tile, in halves: id 41's and id 27's stride
/// at the runtime's pinned tile dimension.
pub const MOVER_IRRADIANCE_TILE_F16_COUNT: usize = DEFAULT_DELTA_PROBE_F16_STRIDE;

/// Most bounce payload bytes a level keeps resident in this section (owner,
/// Stop 2, 2026-11-08). The compiler places a heavier bounce payload with id
/// 50's cells instead and records [`MoverPayloadPlacement::Streamed`].
pub const MOVER_BOUNCE_RESIDENT_BYTE_BAR: u64 = 8 * 1024 * 1024;

/// Most direct payload bytes (visibility and own tiles) a level keeps
/// resident in this section: the bounce bar's sibling, at the same value
/// (owner, 2026-11-08). A heavier direct payload is placed with id 50's
/// cells instead.
pub const MOVER_DIRECT_RESIDENT_BYTE_BAR: u64 = 8 * 1024 * 1024;

/// What a term record's `u32` light index counts in. Each gated light has
/// exactly one name: a static lightmap light with an AlphaLights record is
/// named in [`Self::StaticLight`], one without in [`Self::BakeOnlyStaticLight`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum MoverLightNamespace {
    /// An id-18 AlphaLights index of a static lightmap light (id 35's set).
    StaticLight = 0,
    /// An animated-baked light's roster index: id 45's light numbering, which
    /// is id 34's animation descriptor order.
    AnimatedBaked = 1,
    /// A static lightmap light of id 35 with no AlphaLights record (a
    /// `_bake_only` light), by its rank in this section's own roster: the
    /// namespace-2 lights the section names, in the compiler's static-baked
    /// order (id 35's soft-visibility seed axis). Indices run densely from 0
    /// to the number of distinct namespace-2 lights the section names
    /// ([`MoverProbeChannelSection::bake_only_light_count`]): such a light
    /// leaves no record in any other section, so the section's own roster is
    /// the only bound load data can verify. Every record carries its own
    /// tile, since no id-41 or id-45 entry holds the light's term.
    BakeOnlyStaticLight = 2,
}

impl MoverLightNamespace {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::StaticLight),
            1 => Some(Self::AnimatedBaked),
            2 => Some(Self::BakeOnlyStaticLight),
            _ => None,
        }
    }

    /// Whether every record in this namespace must set
    /// [`MOVER_RECORD_FLAG_OWN_TILE`]: no per-probe entry of another section
    /// can hold the light's term.
    pub fn requires_own_tile(self) -> bool {
        matches!(self, Self::BakeOnlyStaticLight)
    }
}

/// One of the two blocks whose payload has a placement: the direct block's
/// visibility and own tiles, or the bounce block's tiles. Their indexes,
/// records, masks and mover entries always stay in this section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MoverChannelBlock {
    Direct,
    Bounce,
}

impl MoverChannelBlock {
    pub const BOTH: [Self; 2] = [Self::Direct, Self::Bounce];

    /// The block's resident byte bar.
    pub fn resident_byte_bar(self) -> u64 {
        match self {
            Self::Direct => MOVER_DIRECT_RESIDENT_BYTE_BAR,
            Self::Bounce => MOVER_BOUNCE_RESIDENT_BYTE_BAR,
        }
    }
}

impl std::fmt::Display for MoverChannelBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Direct => "direct",
            Self::Bounce => "bounce",
        })
    }
}

/// Where a block's payload lives. Placement is residency only: a streamed
/// payload is the same bytes, placed with id 50's cells and read with them,
/// never a switch driven by a light or a mover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MoverPayloadPlacement {
    /// The payload follows its block's index in this section.
    Resident = 0,
    /// The payload is absent here and streams with id 50's cells, whose
    /// source list names this section. Needs at least one entry in its
    /// block.
    Streamed = 1,
}

impl MoverPayloadPlacement {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Resident),
            1 => Some(Self::Streamed),
            _ => None,
        }
    }
}

/// One listed affinity cell: a cell holding a direct record or a bounce
/// entry. Cells with neither are not listed, so the index grows with the
/// cells movers change, never with the map's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoverChannelCell {
    /// Dense affinity cell index, x-fastest over `affinity_dims`.
    pub cell: u32,
    /// Local x-fastest bits of the cell's valid probes: id 34's validity.
    pub valid_probe_mask: u64,
}

/// One gated light's term at one listed cell, shared by every mover that
/// gates it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoverDirectRecord {
    pub namespace: MoverLightNamespace,
    /// Wire flag bit 0 ([`MOVER_RECORD_FLAG_OWN_TILE`]).
    pub carries_own_tile: bool,
    pub light_index: u32,
    /// Storage sparsity mask: local x-fastest bits, a non-empty subset of the
    /// cell's valid probes, set exactly where some mover's `q₀` or `q₁` is
    /// below 255. A probe outside it stores nothing because every mover
    /// leaves the light's term there unchanged (q = 1 at both termini), so it
    /// composes exactly as with no section. It is never a runtime exclusion
    /// or occlusion flag: mover state never marks a probe occluded, forces a
    /// probe weight to zero or drops a probe from the blend.
    pub probe_mask: u64,
}

impl MoverDirectRecord {
    /// Records ascend by this key within a cell, with no repeats.
    pub fn order_key(&self) -> (MoverLightNamespace, u32) {
        (self.namespace, self.light_index)
    }

    /// Probes the record stores: `q` pairs per mover and, if flagged, a tile.
    pub fn masked_probe_count(&self) -> usize {
        self.probe_mask.count_ones() as usize
    }
}

/// One mover at one listed cell: the probes whose static bounce it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoverBounceEntry {
    pub mover_id: u32,
    /// Local x-fastest probe bits, a non-empty subset of the cell's valid
    /// probes.
    pub affected_probe_mask: u64,
}

/// One mover's closed-pose depth at one probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MoverDepthRecord {
    pub mover_id: u32,
    /// Dense id-34 lattice index of a valid probe.
    pub probe_index: u32,
}

/// A mover's author light ramp: its light state reads
/// `clamp((t - start) / (end - start), 0, 1)`, with `0 <= start < end <= 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoverLightRampRecord {
    pub mover_id: u32,
    pub start: f32,
    pub end: f32,
}

/// MoverProbeChannel section (ID 57), epoch 3. Optional: written only when
/// the direct, bounce or depth block has an entry; never written empty.
///
/// On-disk layout (all little-endian, f16 as raw `u16` bits). N is the
/// listed cell count, R the term record count, M the mover entry count:
///
/// ```text
///   u8        epoch                       (= MOVER_PROBE_CHANNEL_EPOCH = 3)
///   u8        affinity_factor             (= AFFINITY_FACTOR = 4)
///   u32 × 3   affinity_dims               (id 34's grid / 4, rounded up)
///   u8        direct_placement            (0 resident, 1 streamed with id 50)
///   u8        bounce_placement            (0 resident, 1 streamed with id 50)
/// cell index
///   u32                                   N
///   per listed cell, 12 bytes:            u32 cell (dense affinity index),
///                                         u64 valid_probe_mask (id 34)
/// direct block
///   u32 × (N + 1)                         direct_offsets (records per listed
///                                         cell, CSR; last = R)
///   per term record, 16 bytes:            u8 namespace (0 AlphaLights, 1
///                                         animated roster, 2 bake-only
///                                         roster), u8 flags (bit 0 own
///                                         tile), u16 reserved 0, u32 light,
///                                         u64 probe_mask
///   u32 × (R + 1)                         mover_offsets (mover entries per
///                                         record, CSR; last = M)
///   u32 × M                               direct_movers (mover ids)
///   u8 × Σ entries 2 × popcount(mask)     visibility (q₀, q₁ per masked
///                                         probe, entry after entry; resident
///                                         only)
///   f16 × Σ flagged popcount(mask) × 108  own_tiles (one RGB tile per masked
///                                         probe, flagged record after record;
///                                         resident only)
/// bounce block
///   u32 × (N + 1)                         bounce_offsets (CSR; last = entries)
///   per bounce entry, 16 bytes:           u32 mover_id, u32 reserved 0,
///                                         u64 affected_probe_mask
///   f16 × Σ popcount(affected) × 2 × 108  bounce_tiles (S^b₀ then S^b₁ per
///                                         affected probe; resident only)
/// depth block
///   u32                                   depth record count
///   per depth record, 8 bytes:            u32 mover_id, u32 probe_index
///   [f16; 2] × records × 2 × 100          depth_tiles (D₀ then D₁ per record)
/// ramps
///   per mover with any entry, 12 bytes:   u32 mover_id, f32 start, f32 end
/// ```
///
/// Listed cells ascend with no repeats, and each holds a direct record or a
/// bounce entry. Records ascend by (namespace, light) within a cell, each
/// gated by at least one mover; a record's movers ascend by id, and each
/// stores some `q` below 255. Masks are non-empty subsets of their cell's
/// valid mask, and a record's mask is exactly the union of its movers'
/// `q < 255` probes. Namespace-2 records always set the own-tile flag.
/// Bounce entries ascend by mover, depth records by (mover, probe), ramp
/// records by mover; none repeats. Ramp records cover exactly the movers
/// named anywhere else. No sentinels: every reserved field is zero and every
/// byte is accounted for.
///
/// A streamed block's payload is placed with id 50's cells, one row per
/// listed cell ([`streamed`]). In memory, a block's payload vectors are held
/// when it is resident, when the compiler built the section (the wire omits
/// a streamed payload), or once a load attaches it from id 50
/// ([`Self::attach_streamed_payload`]); otherwise they are empty.
#[derive(Debug, Clone, PartialEq)]
pub struct MoverProbeChannelSection {
    pub affinity_factor: u8,
    pub affinity_dims: [u32; 3],
    pub direct_placement: MoverPayloadPlacement,
    pub bounce_placement: MoverPayloadPlacement,
    /// Cells holding a direct record or bounce entry, ascending.
    pub cells: Vec<MoverChannelCell>,
    /// Term records per listed cell (CSR, `cells.len() + 1`).
    pub direct_offsets: Vec<u32>,
    pub direct_records: Vec<MoverDirectRecord>,
    /// Mover entries per record (CSR, `direct_records.len() + 1`).
    pub mover_offsets: Vec<u32>,
    /// Each mover entry's id 43 `mover_id`.
    pub direct_movers: Vec<u32>,
    /// Mover entry after mover entry: `q₀, q₁` per masked probe of its record.
    /// Empty while a streamed direct payload is not held.
    pub visibility: Vec<u8>,
    /// Flagged records only, in record order: one tile per masked probe.
    /// Empty while a streamed direct payload is not held.
    pub own_tiles: Vec<u16>,
    /// Bounce entries per listed cell (CSR, `cells.len() + 1`).
    pub bounce_offsets: Vec<u32>,
    pub bounce_entries: Vec<MoverBounceEntry>,
    /// Entry after entry: per affected probe, `S^b₀` then `S^b₁`. Empty while
    /// a streamed bounce payload is not held.
    pub bounce_tiles: Vec<u16>,
    pub depth_records: Vec<MoverDepthRecord>,
    /// Record after record: `D₀` then `D₁`, each id 59's bordered tile.
    pub depth_tiles: Vec<[u16; 2]>,
    pub ramps: Vec<MoverLightRampRecord>,
}

impl MoverProbeChannelSection {
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims.iter().map(|&d| d as usize).product()
    }

    /// Position of affinity cell `cell` in [`Self::cells`], if listed.
    pub fn listed_position(&self, cell: usize) -> Option<usize> {
        let cell = u32::try_from(cell).ok()?;
        self.cells
            .binary_search_by_key(&cell, |listed| listed.cell)
            .ok()
    }

    /// Whether affinity cell `cell` holds a direct record or bounce entry.
    /// Such a cell composes at L0, so the base never stores it coarser there.
    pub fn cell_has_entries(&self, cell: usize) -> bool {
        self.listed_position(cell).is_some()
    }

    /// Whether affinity cell `cell` holds a direct record, which caps id 41's
    /// level there.
    pub fn cell_has_direct_records(&self, cell: usize) -> bool {
        self.listed_position(cell)
            .is_some_and(|at| self.direct_offsets[at] != self.direct_offsets[at + 1])
    }

    /// Every listed affinity cell, ascending.
    pub fn listed_cells(&self) -> impl Iterator<Item = usize> + '_ {
        self.cells.iter().map(|listed| listed.cell as usize)
    }

    /// Movers gating record `record`, ascending by id.
    pub fn record_movers(&self, record: usize) -> &[u32] {
        let start = self.mover_offsets[record] as usize;
        let end = self.mover_offsets[record + 1] as usize;
        &self.direct_movers[start..end]
    }

    /// `[D₀, D₁]` of depth record `record`, each a bordered id-59 tile.
    pub fn depth_tile_pair(&self, record: usize) -> [&[[u16; 2]]; 2] {
        let start = record * 2 * PROBE_DEPTH_TILE_TEXELS;
        [
            &self.depth_tiles[start..start + PROBE_DEPTH_TILE_TEXELS],
            &self.depth_tiles[start + PROBE_DEPTH_TILE_TEXELS..start + 2 * PROBE_DEPTH_TILE_TEXELS],
        ]
    }

    /// The author ramp of `mover_id`, if the section names that mover.
    pub fn ramp_of(&self, mover_id: u32) -> Option<&MoverLightRampRecord> {
        self.ramps
            .binary_search_by_key(&mover_id, |ramp| ramp.mover_id)
            .ok()
            .map(|index| &self.ramps[index])
    }

    /// Every mover any block names, ascending: the movers ramp records cover.
    pub fn mover_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .direct_movers
            .iter()
            .copied()
            .chain(self.bounce_entries.iter().map(|entry| entry.mover_id))
            .chain(self.depth_records.iter().map(|record| record.mover_id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Distinct [`MoverLightNamespace::BakeOnlyStaticLight`] lights the
    /// direct block names: the roster size that bounds namespace 2's indices.
    pub fn bake_only_light_count(&self) -> usize {
        let mut lights: Vec<u32> = self
            .direct_records
            .iter()
            .filter(|record| record.namespace == MoverLightNamespace::BakeOnlyStaticLight)
            .map(|record| record.light_index)
            .collect();
        lights.sort_unstable();
        lights.dedup();
        lights.len()
    }

    /// Probes carrying mover depth: one renderer-owned live tile each, however
    /// many movers change the probe.
    pub fn live_tile_count(&self) -> usize {
        let mut probes: Vec<u32> = self
            .depth_records
            .iter()
            .map(|record| record.probe_index)
            .collect();
        probes.sort_unstable();
        probes.dedup();
        probes.len()
    }

    /// Bytes the bounce payload weighs wherever it is placed: what the
    /// compiler holds against [`MOVER_BOUNCE_RESIDENT_BYTE_BAR`].
    pub fn bounce_payload_bytes(&self) -> u64 {
        self.bounce_entries
            .iter()
            .map(|entry| bounce_entry_f16_count(entry) as u64 * 2)
            .sum()
    }

    /// Bytes the direct payload (visibility and own tiles) weighs wherever it
    /// is placed: what the compiler holds against
    /// [`MOVER_DIRECT_RESIDENT_BYTE_BAR`].
    pub fn direct_payload_bytes(&self) -> u64 {
        let counts = self.direct_counts();
        (counts.visibility_pairs * MOVER_VISIBILITY_BYTES_PER_PROBE
            + counts.own_tile_probes * MOVER_IRRADIANCE_TILE_F16_COUNT * 2) as u64
    }

    /// `block`'s payload bytes wherever it is placed.
    pub fn payload_bytes(&self, block: MoverChannelBlock) -> u64 {
        match block {
            MoverChannelBlock::Direct => self.direct_payload_bytes(),
            MoverChannelBlock::Bounce => self.bounce_payload_bytes(),
        }
    }

    pub fn placement(&self, block: MoverChannelBlock) -> MoverPayloadPlacement {
        match block {
            MoverChannelBlock::Direct => self.direct_placement,
            MoverChannelBlock::Bounce => self.bounce_placement,
        }
    }

    /// Whether either block's payload streams with id 50: then id 50 lists
    /// this section as a source and id 49 as a resource.
    pub fn streams(&self) -> bool {
        MoverChannelBlock::BOTH
            .iter()
            .any(|&block| self.placement(block) == MoverPayloadPlacement::Streamed)
    }

    /// Whether `block`'s payload is held in memory: resident, built by the
    /// compiler, or attached from id 50. A block with no entry holds an
    /// empty payload, which counts as held.
    pub fn payload_held(&self, block: MoverChannelBlock) -> bool {
        if self.placement(block) == MoverPayloadPlacement::Resident {
            return true;
        }
        match block {
            MoverChannelBlock::Direct => {
                self.direct_records.is_empty() || !self.visibility.is_empty()
            }
            MoverChannelBlock::Bounce => {
                self.bounce_entries.is_empty() || !self.bounce_tiles.is_empty()
            }
        }
    }

    /// Bytes [`Self::to_bytes`] writes.
    pub fn encoded_len(&self) -> usize {
        self.byte_breakdown().total()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        wire::to_bytes(self)
    }

    /// Structural decode: epoch first, then every count, order, mask,
    /// reserved field and length the layout pins. Checks against other
    /// sections (id 34's grid and validity, id 43's movers, light namespaces,
    /// id 59's tile count) are the loader's.
    pub fn from_bytes(data: &[u8]) -> Result<Self, MoverProbeChannelError> {
        wire::from_bytes(data)
    }
}

/// f16 halves one bounce entry's payload holds: two tiles per affected probe.
fn bounce_entry_f16_count(entry: &MoverBounceEntry) -> usize {
    entry.affected_probe_mask.count_ones() as usize * 2 * MOVER_IRRADIANCE_TILE_F16_COUNT
}

/// The listed-cell position of each CSR entry, in entry order.
fn entry_positions(offsets: &[u32]) -> impl Iterator<Item = usize> + '_ {
    offsets
        .windows(2)
        .enumerate()
        .flat_map(|(at, range)| std::iter::repeat_n(at, range[1].saturating_sub(range[0]) as usize))
}

#[cfg(test)]
mod tests;
