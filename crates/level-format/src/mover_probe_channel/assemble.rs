// MoverProbeChannel (id 57) assembly: lay a producer's records, bounce
// entries and depth records out in wire order over the sparse cell list.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE, MoverBounceEntry,
    MoverChannelCell, MoverDepthRecord, MoverDirectRecord, MoverLightRampRecord,
    MoverPayloadPlacement, MoverProbeChannelSection, bounce_entry_f16_count,
};
use crate::delta_sh_volumes::AFFINITY_FACTOR;
use crate::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

/// One term record and its payload, in any order among its siblings.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectRecordParts {
    /// Dense affinity cell index.
    pub cell: u32,
    pub record: MoverDirectRecord,
    /// Each gating mover with `q₀, q₁` per masked probe, in any order.
    pub movers: Vec<(u32, Vec<u8>)>,
    /// One tile per masked probe when the record carries its own; else empty.
    pub own_tiles: Vec<u16>,
}

/// One bounce entry and its payload: per affected probe, x-fastest, its
/// stored `S^b₀` then its stored `S^b₁`.
#[derive(Debug, Clone, PartialEq)]
pub struct BounceEntryParts {
    pub cell: u32,
    pub entry: MoverBounceEntry,
    pub tiles: Vec<u16>,
}

/// One depth record and its stored `D₀` then its stored `D₁`.
#[derive(Debug, Clone, PartialEq)]
pub struct DepthRecordParts {
    pub record: MoverDepthRecord,
    pub tiles: Vec<[u16; 2]>,
}

/// Everything id 57 holds before layout, every payload included: the
/// assembled section holds both payloads resident, and the producer places a
/// block with id 50 afterwards by setting its placement.
#[derive(Debug, Clone, PartialEq)]
pub struct MoverProbeChannelParts {
    pub affinity_dims: [u32; 3],
    pub direct: Vec<DirectRecordParts>,
    pub bounce: Vec<BounceEntryParts>,
    pub depth: Vec<DepthRecordParts>,
}

impl MoverProbeChannelSection {
    /// Lay `parts` out in wire order: cells ascending, records by
    /// (namespace, light) within a cell, movers by id within a record,
    /// bounce entries by mover, depth records by (mover, probe). Every cell
    /// a record or bounce entry names is listed, with `valid_probe_mask`'s
    /// mask; `ramp_of` gives each named mover's ramp. `None` when no block
    /// holds anything: the section is never written empty.
    ///
    /// The canonical content is the producer's: each record's mask must be
    /// exactly its movers' `q < 255` probes, and no record, mover entry or
    /// bounce entry may repeat. Decoding refuses a section that breaks them.
    pub fn assemble<E>(
        parts: MoverProbeChannelParts,
        valid_probe_mask: impl Fn(u32) -> u64,
        mut ramp_of: impl FnMut(u32) -> Result<MoverLightRampRecord, E>,
    ) -> Result<Option<Self>, E> {
        let MoverProbeChannelParts {
            affinity_dims,
            mut direct,
            mut bounce,
            mut depth,
        } = parts;
        if direct.is_empty() && bounce.is_empty() && depth.is_empty() {
            return Ok(None);
        }
        direct.sort_by_key(|part| (part.cell, part.record.order_key()));
        bounce.sort_by_key(|part| (part.cell, part.entry.mover_id));
        depth.sort_by_key(|part| part.record.order_key());

        let mut listed: Vec<u32> = direct
            .iter()
            .map(|part| part.cell)
            .chain(bounce.iter().map(|part| part.cell))
            .collect();
        listed.sort_unstable();
        listed.dedup();
        let cells: Vec<MoverChannelCell> = listed
            .iter()
            .map(|&cell| MoverChannelCell {
                cell,
                valid_probe_mask: valid_probe_mask(cell),
            })
            .collect();

        let mut section = MoverProbeChannelSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            direct_placement: MoverPayloadPlacement::Resident,
            bounce_placement: MoverPayloadPlacement::Resident,
            direct_offsets: csr(&listed, direct.iter().map(|part| part.cell)),
            bounce_offsets: csr(&listed, bounce.iter().map(|part| part.cell)),
            cells,
            direct_records: Vec::with_capacity(direct.len()),
            mover_offsets: vec![0],
            direct_movers: Vec::new(),
            visibility: Vec::new(),
            own_tiles: Vec::new(),
            bounce_entries: Vec::with_capacity(bounce.len()),
            bounce_tiles: Vec::new(),
            depth_records: Vec::with_capacity(depth.len()),
            depth_tiles: Vec::new(),
            ramps: Vec::new(),
        };
        for mut part in direct {
            let probes = part.record.masked_probe_count();
            debug_assert_eq!(
                part.own_tiles.len(),
                if part.record.carries_own_tile {
                    probes * MOVER_IRRADIANCE_TILE_F16_COUNT
                } else {
                    0
                },
                "own tiles cover exactly the masked probes of a flagged record"
            );
            part.movers.sort_by_key(|(mover_id, _)| *mover_id);
            for (mover_id, visibility) in &part.movers {
                debug_assert_eq!(visibility.len(), probes * MOVER_VISIBILITY_BYTES_PER_PROBE);
                section.direct_movers.push(*mover_id);
                section.visibility.extend_from_slice(visibility);
            }
            section
                .mover_offsets
                .push(u32::try_from(section.direct_movers.len()).expect("mover entries fit u32"));
            section.own_tiles.extend_from_slice(&part.own_tiles);
            section.direct_records.push(part.record);
        }
        for part in bounce {
            debug_assert_eq!(part.tiles.len(), bounce_entry_f16_count(&part.entry));
            section.bounce_tiles.extend_from_slice(&part.tiles);
            section.bounce_entries.push(part.entry);
        }
        for part in depth {
            debug_assert_eq!(
                part.tiles.len(),
                part.record.stored_tile_count() * PROBE_DEPTH_TILE_TEXELS
            );
            section.depth_tiles.extend_from_slice(&part.tiles);
            section.depth_records.push(part.record);
        }
        section.ramps = section
            .mover_ids()
            .into_iter()
            .map(&mut ramp_of)
            .collect::<Result<_, E>>()?;
        Ok(Some(section))
    }
}

/// CSR offsets over `listed` cells of `cells`, which ascends.
fn csr(listed: &[u32], cells: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut offsets = vec![0u32; listed.len() + 1];
    for cell in cells {
        let at = listed
            .binary_search(&cell)
            .expect("every named cell is listed");
        offsets[at + 1] += 1;
    }
    for at in 0..listed.len() {
        offsets[at + 1] += offsets[at];
    }
    offsets
}
