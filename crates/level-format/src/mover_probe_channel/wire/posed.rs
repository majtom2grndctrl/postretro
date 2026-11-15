// MoverProbeChannel (id 57) per-pose bytes: mover records with their knots,
// variable-length bounce entries behind byte offsets, and depth records'
// two-bit pose marks, each decoded against its mover's pose count.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::reader::{Reader, le_f32, le_u32, le_u64, put_u32s};
use super::{Error, MoverBounceEntry, MoverDepthRecord, MoverRecord};
use crate::mover_probe_channel::{BouncePoseMasks, PoseMark, is_terminus};

/// u32 mover, f32 ramp start, f32 ramp end, u32 pose count; knots follow.
const MOVER_RECORD_HEAD_BYTES: usize = 16;
/// u32 mover, u32 reserved; one (stored, base) mask pair per pose follows.
const BOUNCE_ENTRY_HEAD_BYTES: usize = 8;
/// u64 stored mask, u64 base mask.
const BOUNCE_POSE_BYTES: usize = 16;
/// u32 mover, u32 probe; the pose-mark words follow.
const DEPTH_RECORD_HEAD_BYTES: usize = 8;
/// Two-bit pose marks per mark word.
const MARKS_PER_WORD: usize = 16;
const MARK_STORED: u32 = 0b01;
const MARK_BASE: u32 = 0b10;

pub(super) fn mover_record_bytes(record: &MoverRecord) -> usize {
    MOVER_RECORD_HEAD_BYTES + 4 * record.pose_count()
}

pub(super) fn bounce_entry_bytes(pose_count: usize) -> usize {
    BOUNCE_ENTRY_HEAD_BYTES + BOUNCE_POSE_BYTES * pose_count
}

/// Bytes one depth record of a `pose_count`-pose mover takes: its marks pad
/// to whole words, so a mover's records share one stride.
pub(super) fn depth_record_bytes(pose_count: usize) -> usize {
    DEPTH_RECORD_HEAD_BYTES + 4 * pose_count.div_ceil(MARKS_PER_WORD)
}

pub(super) fn put_mover_records(out: &mut Vec<u8>, records: &[MoverRecord]) {
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for record in records {
        out.extend_from_slice(&record.mover_id.to_le_bytes());
        out.extend_from_slice(&record.ramp_start.to_le_bytes());
        out.extend_from_slice(&record.ramp_end.to_le_bytes());
        out.extend_from_slice(&(record.pose_count() as u32).to_le_bytes());
        for knot in &record.knots {
            out.extend_from_slice(&knot.to_le_bytes());
        }
    }
}

/// The entry byte offsets, then each entry.
pub(super) fn put_bounce_entries(out: &mut Vec<u8>, entries: &[MoverBounceEntry]) {
    let mut offset = 0u32;
    let mut offsets = Vec::with_capacity(entries.len() + 1);
    offsets.push(offset);
    for entry in entries {
        let bytes = u32::try_from(bounce_entry_bytes(entry.pose_count()))
            .expect("a bounce entry's bytes fit u32");
        offset = offset
            .checked_add(bytes)
            .expect("the bounce entries' bytes fit u32");
        offsets.push(offset);
    }
    put_u32s(out, &offsets);
    for entry in entries {
        out.extend_from_slice(&entry.mover_id.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for pose in &entry.poses {
            out.extend_from_slice(&pose.stored.to_le_bytes());
            out.extend_from_slice(&pose.base.to_le_bytes());
        }
    }
}

pub(super) fn put_depth_records(out: &mut Vec<u8>, records: &[MoverDepthRecord]) {
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for record in records {
        out.extend_from_slice(&record.mover_id.to_le_bytes());
        out.extend_from_slice(&record.probe_index.to_le_bytes());
        let mut words = vec![0u32; record.pose_count().div_ceil(MARKS_PER_WORD)];
        for (pose, mark) in record.marks.iter().enumerate() {
            let bits = match mark {
                PoseMark::Unmarked => 0,
                PoseMark::Stored => MARK_STORED,
                PoseMark::Base => MARK_BASE,
            };
            words[pose / MARKS_PER_WORD] |= bits << (2 * (pose % MARKS_PER_WORD));
        }
        put_u32s(out, &words);
    }
}

/// Mover records: ascending by id, each with a valid ramp, at least two
/// poses, and knots running strictly ascending from exactly 0 to exactly 1.
pub(super) fn read_mover_records(reader: &mut Reader<'_>) -> Result<Vec<MoverRecord>, Error> {
    let count = reader.u32("mover record count")? as usize;
    // Each record takes at least its head, so a count past the bytes fails
    // on its first short read, never on an allocation.
    let mut records: Vec<MoverRecord> = Vec::new();
    for record in 0..count {
        let head = reader.take(MOVER_RECORD_HEAD_BYTES, "mover records")?;
        let mover_id = le_u32(&head[0..4]);
        let (ramp_start, ramp_end) = (le_f32(&head[4..8]), le_f32(&head[8..12]));
        let pose_count = le_u32(&head[12..16]);
        if records
            .last()
            .is_some_and(|previous| previous.mover_id >= mover_id)
        {
            return Err(Error::MoverRecordOrder { record });
        }
        // Every comparison is false for NaN, and infinities fail the bounds.
        if !(ramp_start >= 0.0 && ramp_start < ramp_end && ramp_end <= 1.0) {
            return Err(Error::RampRange {
                mover_id,
                start: ramp_start,
                end: ramp_end,
            });
        }
        if pose_count < 2 {
            return Err(Error::PoseCount {
                mover_id,
                count: pose_count,
            });
        }
        let knots: Vec<f32> = reader
            .records(pose_count as usize, 4, "mover knots")?
            .map(le_f32)
            .collect();
        check_knots(mover_id, &knots)?;
        records.push(MoverRecord {
            mover_id,
            ramp_start,
            ramp_end,
            knots,
        });
    }
    Ok(records)
}

fn check_knots(mover_id: u32, knots: &[f32]) -> Result<(), Error> {
    for (knot, &value) in knots.iter().enumerate() {
        if !value.is_finite() {
            return Err(Error::KnotNotFinite {
                mover_id,
                knot,
                value,
            });
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::KnotOutOfRange {
                mover_id,
                knot,
                value,
            });
        }
        if let Some(&previous) = knot.checked_sub(1).map(|at| &knots[at])
            && value <= previous
        {
            return Err(Error::KnotOrder {
                mover_id,
                knot,
                value,
                previous,
            });
        }
    }
    let (first, last) = (knots[0], knots[knots.len() - 1]);
    // Bits, not value: −0.0 equals 0.0, and one section has one encoding.
    if first.to_bits() != 0.0f32.to_bits() || last != 1.0 {
        return Err(Error::KnotEnds {
            mover_id,
            first,
            last,
        });
    }
    Ok(())
}

/// `count` bounce entries behind their byte offsets. Each spans exactly its
/// mover's poses; no pose marks a probe both stored and base, and no
/// terminus marks base.
pub(super) fn read_bounce_entries(
    reader: &mut Reader<'_>,
    count: usize,
    pose_count: impl Fn(&'static str, u32) -> Result<usize, Error>,
) -> Result<Vec<MoverBounceEntry>, Error> {
    let offsets = reader.offsets(count, "bounce entry byte", "bounce entry byte offsets")?;
    let bytes = reader.take(
        *offsets.last().expect("count + 1 offsets") as usize,
        "bounce entries",
    )?;
    let mut entries = Vec::with_capacity(count);
    for (index, range) in offsets.windows(2).enumerate() {
        let span = &bytes[range[0] as usize..range[1] as usize];
        if span.len() < BOUNCE_ENTRY_HEAD_BYTES {
            return Err(Error::Truncated {
                what: "bounce entry head",
                needed: BOUNCE_ENTRY_HEAD_BYTES,
                remaining: span.len(),
            });
        }
        let mover_id = le_u32(&span[0..4]);
        let reserved = le_u32(&span[4..8]);
        if reserved != 0 {
            return Err(Error::ReservedBits {
                what: "bounce entry reserved u32",
                index,
                value: reserved,
            });
        }
        let poses = pose_count("bounce", mover_id)?;
        let expected = bounce_entry_bytes(poses);
        if span.len() != expected {
            let pairs = span.len() - BOUNCE_ENTRY_HEAD_BYTES;
            // Whole pose pairs past the mover's count name poses it lacks.
            return Err(
                if span.len() > expected && pairs.is_multiple_of(BOUNCE_POSE_BYTES) {
                    Error::PoseOutOfRange {
                        block: "bounce",
                        what: "entry",
                        index,
                        mover_id,
                        pose_count: poses,
                    }
                } else {
                    Error::BounceEntryLength {
                        entry: index,
                        mover_id,
                        pose_count: poses,
                        bytes: span.len(),
                        expected,
                    }
                },
            );
        }
        let mut masks = Vec::with_capacity(poses);
        for (pose, pair) in span[BOUNCE_ENTRY_HEAD_BYTES..]
            .as_chunks::<BOUNCE_POSE_BYTES>()
            .0
            .iter()
            .enumerate()
        {
            let (stored, base) = (le_u64(&pair[0..8]), le_u64(&pair[8..16]));
            if stored & base != 0 {
                return Err(Error::StoredAndBase {
                    block: "bounce",
                    what: "entry",
                    index,
                    pose,
                    probe: (stored & base).trailing_zeros(),
                });
            }
            if base != 0 && is_terminus(pose, poses) {
                return Err(Error::BaseAtTerminus {
                    block: "bounce",
                    what: "entry",
                    index,
                    pose,
                });
            }
            masks.push(BouncePoseMasks { stored, base });
        }
        entries.push(MoverBounceEntry {
            mover_id,
            poses: masks,
        });
    }
    Ok(entries)
}

/// `count` depth records, each its mover's stride. Every record stores some
/// pose, marks no pose both stored and base, marks base at no terminus, and
/// sets no mark past its mover's poses.
pub(super) fn read_depth_records(
    reader: &mut Reader<'_>,
    count: usize,
    pose_count: impl Fn(&'static str, u32) -> Result<usize, Error>,
) -> Result<Vec<MoverDepthRecord>, Error> {
    // Each record takes at least its head, so a count past the bytes fails
    // on its first short read, never on an allocation.
    let mut records = Vec::new();
    for index in 0..count {
        let head = reader.take(DEPTH_RECORD_HEAD_BYTES, "depth records")?;
        let (mover_id, probe_index) = (le_u32(&head[0..4]), le_u32(&head[4..8]));
        let poses = pose_count("depth", mover_id)?;
        let words: Vec<u32> = reader
            .records(poses.div_ceil(MARKS_PER_WORD), 4, "depth pose marks")?
            .map(le_u32)
            .collect();
        let mut marks = Vec::with_capacity(poses);
        for pose in 0..poses {
            let bits = (words[pose / MARKS_PER_WORD] >> (2 * (pose % MARKS_PER_WORD))) & 0b11;
            marks.push(match bits {
                0 => PoseMark::Unmarked,
                MARK_STORED => PoseMark::Stored,
                MARK_BASE if is_terminus(pose, poses) => {
                    return Err(Error::BaseAtTerminus {
                        block: "depth",
                        what: "record",
                        index,
                        pose,
                    });
                }
                MARK_BASE => PoseMark::Base,
                _ => {
                    return Err(Error::StoredAndBase {
                        block: "depth",
                        what: "record",
                        index,
                        pose,
                        probe: probe_index,
                    });
                }
            });
        }
        let used = 2 * (poses % MARKS_PER_WORD);
        let last = words.last().copied().unwrap_or(0);
        if used != 0 && last >> used != 0 {
            return Err(Error::PoseOutOfRange {
                block: "depth",
                what: "record",
                index,
                mover_id,
                pose_count: poses,
            });
        }
        if !marks.contains(&PoseMark::Stored) {
            return Err(Error::DepthRecordStoresNothing { record: index });
        }
        records.push(MoverDepthRecord {
            mover_id,
            probe_index,
            marks,
        });
    }
    Ok(records)
}
