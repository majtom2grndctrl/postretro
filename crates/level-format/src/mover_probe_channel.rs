// MoverProbeChannel PRL section (ID 57): what each analysed mover changes in
// the probe data at its two termini (per-light direct visibility, absolute
// static bounce, closed-pose depth tiles) and each mover's author light ramp.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

mod error;
mod wire;

pub use error::MoverProbeChannelError;

use crate::delta_sh_volumes::DEFAULT_DELTA_PROBE_F16_STRIDE;
use crate::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

/// Section-internal epoch, the first byte of the payload. Exact match.
pub const MOVER_PROBE_CHANNEL_EPOCH: u8 = 1;

/// Direct-entry flag bit 0: the entry carries its own tile per valid probe,
/// because no per-probe id-41 or id-45 entry holds the light's term there.
/// Every other flag bit is reserved and refused.
pub const MOVER_ENTRY_FLAG_OWN_TILE: u8 = 1 << 0;

/// Visibility bytes per (direct entry, valid probe): `q₀` then `q₁`, each
/// `byte / 255`.
pub const MOVER_VISIBILITY_BYTES_PER_PROBE: usize = 2;

/// One compact RGB f16 octahedral tile, in halves: id 41's and id 27's stride
/// at the runtime's pinned tile dimension.
pub const MOVER_IRRADIANCE_TILE_F16_COUNT: usize = DEFAULT_DELTA_PROBE_F16_STRIDE;

/// Most bounce payload bytes a level keeps resident in this section (owner,
/// 2026-11-08). The compiler places a heavier bounce payload with id 50's
/// cells instead and records [`MoverBouncePlacement::Streamed`].
pub const MOVER_BOUNCE_RESIDENT_BYTE_BAR: u64 = 8 * 1024 * 1024;

/// What a direct entry's `u32` light index counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum MoverLightNamespace {
    /// An id-18 AlphaLights index of a static lightmap light (id 35's set).
    StaticLight = 0,
    /// An animated-baked light's roster index: id 45's light numbering, which
    /// is id 34's animation descriptor order.
    AnimatedBaked = 1,
}

impl MoverLightNamespace {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::StaticLight),
            1 => Some(Self::AnimatedBaked),
            _ => None,
        }
    }
}

/// Where the bounce payload lives. The bounce index stays in this section
/// either way, so mover membership never needs id 50.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MoverBouncePlacement {
    /// The payload follows the bounce index in this section.
    Resident = 0,
    /// The payload is absent here and streams with id 50's cells, whose
    /// source list names this section. Needs at least one bounce entry.
    Streamed = 1,
}

impl MoverBouncePlacement {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Resident),
            1 => Some(Self::Streamed),
            _ => None,
        }
    }
}

/// One (mover, light) at one affinity cell: the light's surviving soft
/// visibility at each terminus for every valid probe of the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoverDirectEntry {
    /// Id 43's `mover_id`.
    pub mover_id: u32,
    pub namespace: MoverLightNamespace,
    /// Wire flag bit 0 ([`MOVER_ENTRY_FLAG_OWN_TILE`]).
    pub carries_own_tile: bool,
    pub light_index: u32,
}

impl MoverDirectEntry {
    /// Entries ascend by this key within a cell, with no repeats.
    pub fn order_key(&self) -> (u32, MoverLightNamespace, u32) {
        (self.mover_id, self.namespace, self.light_index)
    }
}

/// One mover at one affinity cell: the probes whose static bounce it changes.
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

/// MoverProbeChannel section (ID 57), epoch 1. Optional: written only when
/// the direct, bounce or depth block has an entry; never written empty.
///
/// On-disk layout (all little-endian, f16 as raw `u16` bits):
///
/// ```text
///   u8        epoch                       (= MOVER_PROBE_CHANNEL_EPOCH)
///   u8        affinity_factor             (= AFFINITY_FACTOR = 4)
///   u32 × 3   affinity_dims               (id 35's grid / 4, rounded up)
///   u8        bounce_placement            (0 resident, 1 streamed with id 50)
///   u64 × cells                           valid_probe_masks (id 34's validity)
///   u32 × (cells + 1)                     direct_offsets (CSR; last = entries)
///   per direct entry, 12 bytes:           u32 mover_id, u8 namespace,
///                                         u8 flags, u16 reserved 0, u32 light
///   u8 × Σ entries 2 × popcount(mask)     visibility (q₀, q₁ per valid probe)
///   f16 × Σ flagged popcount(mask) × 108  own_tiles (one RGB tile per probe)
///   u32 × (cells + 1)                     bounce_offsets (CSR)
///   per bounce entry, 16 bytes:           u32 mover_id, u32 reserved 0,
///                                         u64 affected_probe_mask
///   f16 × Σ popcount(affected) × 2 × 108  bounce_tiles (S^b₀ then S^b₁ per
///                                         affected probe; resident only)
///   u32                                   depth record count
///   per depth record, 8 bytes:            u32 mover_id, u32 probe_index
///   [f16; 2] × records × 2 × 100          depth_tiles (D₀ then D₁ per record)
///   per mover with any entry, 12 bytes:   u32 mover_id, f32 start, f32 end
/// ```
///
/// Direct entries ascend by (mover, namespace, light) within a cell, bounce
/// entries by mover, depth records by (mover, probe), ramp records by mover;
/// none repeats. Ramp records cover exactly the movers named anywhere else.
/// No sentinels: every reserved field is zero and every byte is accounted for.
#[derive(Debug, Clone, PartialEq)]
pub struct MoverProbeChannelSection {
    pub affinity_factor: u8,
    pub affinity_dims: [u32; 3],
    pub bounce_placement: MoverBouncePlacement,
    /// One valid-probe mask per affinity cell, equal to id 34's validity.
    pub valid_probe_masks: Vec<u64>,
    pub direct_offsets: Vec<u32>,
    pub direct_entries: Vec<MoverDirectEntry>,
    /// Entry after entry: `q₀, q₁` per valid probe of the entry's cell.
    pub visibility: Vec<u8>,
    /// Flagged entries only, in entry order: one tile per valid probe.
    pub own_tiles: Vec<u16>,
    pub bounce_offsets: Vec<u32>,
    pub bounce_entries: Vec<MoverBounceEntry>,
    /// Entry after entry: per affected probe, `S^b₀` then `S^b₁`. Empty when
    /// the payload streams.
    pub bounce_tiles: Vec<u16>,
    pub depth_records: Vec<MoverDepthRecord>,
    /// Record after record: `D₀` then `D₁`, each id 59's bordered tile.
    pub depth_tiles: Vec<[u16; 2]>,
    pub ramps: Vec<MoverLightRampRecord>,
}

/// A direct entry with its payload.
#[derive(Debug, Clone, Copy)]
pub struct DirectEntryView<'a> {
    pub cell: usize,
    pub entry: &'a MoverDirectEntry,
    pub valid_probe_mask: u64,
    /// `q₀, q₁` per valid probe, x-fastest.
    pub visibility: &'a [u8],
    /// One tile per valid probe; `None` unless the entry carries its own.
    pub own_tiles: Option<&'a [u16]>,
}

/// A bounce entry with its payload.
#[derive(Debug, Clone, Copy)]
pub struct BounceEntryView<'a> {
    pub cell: usize,
    pub entry: &'a MoverBounceEntry,
    /// `S^b₀, S^b₁` per affected probe, x-fastest; `None` when the payload
    /// streams with id 50.
    pub tiles: Option<&'a [u16]>,
}

impl MoverProbeChannelSection {
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims.iter().map(|&d| d as usize).product()
    }

    /// Every direct entry with its cell and payload, in wire order.
    pub fn direct_entry_views(&self) -> impl Iterator<Item = DirectEntryView<'_>> {
        let mut visibility_at = 0;
        let mut tile_at = 0;
        entry_cells(&self.direct_offsets)
            .zip(&self.direct_entries)
            .map(move |(cell, entry)| {
                let valid_probe_mask = self.valid_probe_masks[cell];
                let probes = valid_probe_mask.count_ones() as usize;
                let visibility_len = probes * MOVER_VISIBILITY_BYTES_PER_PROBE;
                let visibility = &self.visibility[visibility_at..visibility_at + visibility_len];
                visibility_at += visibility_len;
                let own_tiles = entry.carries_own_tile.then(|| {
                    let len = probes * MOVER_IRRADIANCE_TILE_F16_COUNT;
                    let tiles = &self.own_tiles[tile_at..tile_at + len];
                    tile_at += len;
                    tiles
                });
                DirectEntryView {
                    cell,
                    entry,
                    valid_probe_mask,
                    visibility,
                    own_tiles,
                }
            })
    }

    /// Every bounce entry with its cell and payload, in wire order.
    pub fn bounce_entry_views(&self) -> impl Iterator<Item = BounceEntryView<'_>> {
        let resident = self.bounce_placement == MoverBouncePlacement::Resident;
        let mut tile_at = 0;
        entry_cells(&self.bounce_offsets)
            .zip(&self.bounce_entries)
            .map(move |(cell, entry)| {
                let tiles = resident.then(|| {
                    let len = bounce_entry_f16_count(entry);
                    let tiles = &self.bounce_tiles[tile_at..tile_at + len];
                    tile_at += len;
                    tiles
                });
                BounceEntryView { cell, entry, tiles }
            })
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
            .direct_entries
            .iter()
            .map(|entry| entry.mover_id)
            .chain(self.bounce_entries.iter().map(|entry| entry.mover_id))
            .chain(self.depth_records.iter().map(|record| record.mover_id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
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

    /// Bytes [`Self::to_bytes`] writes.
    pub fn encoded_len(&self) -> usize {
        wire::encoded_len(self)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        wire::to_bytes(self)
    }

    /// Structural decode: epoch first, then every count, order, reserved field
    /// and length the layout pins. Checks against other sections (id 34's
    /// grid and validity, id 43's movers, light namespaces, id 59's tile
    /// count) are the loader's.
    pub fn from_bytes(data: &[u8]) -> Result<Self, MoverProbeChannelError> {
        wire::from_bytes(data)
    }
}

/// f16 halves one bounce entry's payload holds: two tiles per affected probe.
fn bounce_entry_f16_count(entry: &MoverBounceEntry) -> usize {
    entry.affected_probe_mask.count_ones() as usize * 2 * MOVER_IRRADIANCE_TILE_F16_COUNT
}

/// The cell of each CSR entry, in entry order.
fn entry_cells(offsets: &[u32]) -> impl Iterator<Item = usize> + '_ {
    offsets.windows(2).enumerate().flat_map(|(cell, range)| {
        std::iter::repeat_n(cell, range[1].saturating_sub(range[0]) as usize)
    })
}

#[cfg(test)]
mod tests;
