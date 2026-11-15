// Mover probe channel direct payload checks, as a cell loads or arrives:
// every term record's mask tight against its q bytes, and every own tile
// finite. A PRL is accepted or refused alike in every mode.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::mover_probe_channel::{
    DirectRecordView, MoverCellPayload, MoverProbeChannelSection,
};

use super::layout::{MoverChannelError, fail, first_non_finite};

/// Check listed cell `position`'s direct payload as it loads or arrives:
/// every term record's probe mask tight against its q bytes, and every own
/// tile finite. It covers every record, whether the table gates it or leaves
/// it ungated, so a PRL is accepted or refused alike in every mode: the
/// whole load checks every record's tightness at decode or attach.
pub(in crate::render) fn check_direct_cell(
    section: &MoverProbeChannelSection,
    position: usize,
    payload: MoverCellPayload<'_>,
) -> Result<(), MoverChannelError> {
    let Some(view) = section.direct_cell_at(position, payload) else {
        return Ok(());
    };
    let first = section.direct_offsets[position] as usize;
    for (offset, record) in view.records().enumerate() {
        let index = first + offset;
        check_tight(index, record.cell, &record)?;
        if let Some(bad) = record
            .own_tiles
            .and_then(|tiles| first_non_finite(tiles.iter()))
        {
            return fail(format!(
                "affinity cell {}'s direct record {index} holds a non-finite own tile value ({bad:#06x})",
                record.cell
            ));
        }
    }
    Ok(())
}

/// A record's probe mask must be exactly the probes where some gating mover
/// leaves q below 255 at some pose, and each mover must store one such q.
fn check_tight(
    record: usize,
    cell: u32,
    view: &DirectRecordView<'_>,
) -> Result<(), MoverChannelError> {
    let probes = view.record.masked_probe_count();
    let expected = view.visibility_len();
    if view.visibility.len() != expected {
        return fail(format!(
            "affinity cell {cell}'s direct record {record} holds {} visibility byte(s), expected {expected}",
            view.visibility.len()
        ));
    }
    let mut touched = 0u64;
    for mover in view.movers() {
        let mut stores = false;
        for slot in 0..probes {
            let q = mover.probe_q(slot);
            if q.iter().any(|&q| q < u8::MAX) {
                touched |= 1 << slot;
                stores = true;
            }
        }
        if !stores {
            return fail(format!(
                "affinity cell {cell}'s direct record {record} lists mover {} with no q below 255",
                mover.mover_id
            ));
        }
    }
    let all = if probes == 64 {
        u64::MAX
    } else {
        (1u64 << probes) - 1
    };
    if touched != all {
        return fail(format!(
            "affinity cell {cell}'s direct record {record} has a probe mask that is not tight"
        ));
    }
    Ok(())
}
