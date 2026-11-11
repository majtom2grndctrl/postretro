// The bounce memo's payload: one mover's states, pose by pose, and the
// checks a payload must pass before a build trusts it.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark, is_terminus,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

use super::states::{BounceProbe, DepthProbe, MoverBounceStates};
use crate::sh_bake::ProbeGridLayout;

fn mark_byte(mark: PoseMark) -> u8 {
    match mark {
        PoseMark::Unmarked => 0,
        PoseMark::Stored => 1,
        PoseMark::Base => 2,
    }
}

fn mark_of(byte: u8) -> Option<PoseMark> {
    match byte {
        0 => Some(PoseMark::Unmarked),
        1 => Some(PoseMark::Stored),
        2 => Some(PoseMark::Base),
        _ => None,
    }
}

/// `u32` pose count N; `u32` bounce count, then per probe `u32` index, N
/// mark bytes (0 unmarked, 1 stored, 2 base) and 108 little-endian halves
/// per stored pose; `u32` depth count, then per probe `u32` index, N mark
/// bytes and 100 `[f16; 2]` texels per stored pose. The mover id is the
/// memo key's, so it is not stored.
pub(crate) fn encode(states: &MoverBounceStates) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(states.pose_count as u32).to_le_bytes());
    out.extend_from_slice(&(states.bounce.len() as u32).to_le_bytes());
    for probe in &states.bounce {
        out.extend_from_slice(&probe.probe.to_le_bytes());
        out.extend(probe.marks.iter().map(|&mark| mark_byte(mark)));
        for half in &probe.tiles {
            out.extend_from_slice(&half.to_le_bytes());
        }
    }
    out.extend_from_slice(&(states.depth.len() as u32).to_le_bytes());
    for probe in &states.depth {
        out.extend_from_slice(&probe.probe.to_le_bytes());
        out.extend(probe.marks.iter().map(|&mark| mark_byte(mark)));
        for texel in &probe.tiles {
            for half in texel {
                out.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    out
}

/// A probe's N marks: each a known byte, at least one stored, and no
/// terminus reading the base, where an unmarked probe already does.
fn read_marks(reader: &mut Reader<'_>, pose_count: usize) -> Option<Vec<PoseMark>> {
    let marks: Vec<PoseMark> = reader
        .take(pose_count)?
        .iter()
        .map(|&byte| mark_of(byte))
        .collect::<Option<_>>()?;
    let stores = marks.contains(&PoseMark::Stored);
    let base_at_terminus = marks
        .iter()
        .enumerate()
        .any(|(pose, &mark)| mark == PoseMark::Base && is_terminus(pose, pose_count));
    (stores && !base_at_terminus).then_some(marks)
}

fn stored_count(marks: &[PoseMark]) -> usize {
    marks
        .iter()
        .filter(|&&mark| mark == PoseMark::Stored)
        .count()
}

/// The states of a well-formed payload for a mover of `pose_count` poses:
/// the same pose count, probes strictly ascending, each a valid probe of
/// `layout` with well-formed marks, every tile whole, and no trailing bytes.
pub(crate) fn decode(
    bytes: &[u8],
    mover_id: u32,
    pose_count: usize,
    layout: &ProbeGridLayout,
) -> Option<MoverBounceStates> {
    let mut reader = Reader(bytes);
    let valid = |probe: u32| layout.validity.get(probe as usize).is_some_and(|&v| v != 0);
    if reader.u32()? as usize != pose_count || pose_count < 2 {
        return None;
    }
    let count = reader.u32()? as usize;
    let mut bounce: Vec<BounceProbe> = Vec::with_capacity(count.min(bytes.len() / 4));
    for _ in 0..count {
        let probe = reader.u32()?;
        if !valid(probe) || bounce.last().is_some_and(|last| last.probe >= probe) {
            return None;
        }
        let marks = read_marks(&mut reader, pose_count)?;
        let tiles = reader
            .take(stored_count(&marks) * MOVER_IRRADIANCE_TILE_F16_COUNT * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&half| u16::from_le_bytes(half))
            .collect();
        bounce.push(BounceProbe {
            probe,
            marks,
            tiles,
        });
    }
    let count = reader.u32()? as usize;
    let mut depth: Vec<DepthProbe> = Vec::with_capacity(count.min(bytes.len() / 4));
    for _ in 0..count {
        let probe = reader.u32()?;
        if !valid(probe) || depth.last().is_some_and(|last| last.probe >= probe) {
            return None;
        }
        let marks = read_marks(&mut reader, pose_count)?;
        let tiles = reader
            .take(stored_count(&marks) * PROBE_DEPTH_TILE_TEXELS * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|t| {
                [
                    u16::from_le_bytes([t[0], t[1]]),
                    u16::from_le_bytes([t[2], t[3]]),
                ]
            })
            .collect();
        depth.push(DepthProbe {
            probe,
            marks,
            tiles,
        });
    }
    reader
        .0
        .is_empty()
        .then(|| MoverBounceStates::new(mover_id, pose_count, bounce, depth))
}

struct Reader<'b>(&'b [u8]);

impl<'b> Reader<'b> {
    fn take(&mut self, len: usize) -> Option<&'b [u8]> {
        if self.0.len() < len {
            return None;
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[cfg(test)]
mod tests {
    use PoseMark::{Base, Stored, Unmarked};
    use glam::DVec3;

    use super::*;

    fn layout() -> ProbeGridLayout {
        ProbeGridLayout {
            world_min: DVec3::ZERO,
            dims: [4, 1, 1],
            cell_size: [1.0; 3],
            far_sentinel: 100.0,
            probe_positions: vec![DVec3::ZERO; 4],
            validity: vec![1, 1, 0, 1],
        }
    }

    fn states() -> MoverBounceStates {
        let bounce = vec![
            BounceProbe {
                probe: 0,
                marks: vec![Stored, Base, Stored],
                tiles: (0..2 * MOVER_IRRADIANCE_TILE_F16_COUNT as u16).collect(),
            },
            BounceProbe {
                probe: 3,
                marks: vec![Stored, Unmarked, Unmarked],
                tiles: vec![7; MOVER_IRRADIANCE_TILE_F16_COUNT],
            },
        ];
        let depth = vec![DepthProbe {
            probe: 1,
            marks: vec![Unmarked, Stored, Stored],
            tiles: vec![[1, 2]; 2 * PROBE_DEPTH_TILE_TEXELS],
        }];
        MoverBounceStates::new(5, 3, bounce, depth)
    }

    // The memo keeps each probe's marks and only its stored poses' tiles,
    // and refuses another pose count, a probe storing nothing, a terminus
    // reading the base, an unknown mark or a torn tile run.
    #[test]
    fn the_memo_round_trips_per_pose_marks_and_refuses_malformed_ones() {
        let states = states();
        let bytes = encode(&states);
        assert_eq!(decode(&bytes, 5, 3, &layout()), Some(states));
        assert_eq!(decode(&bytes, 5, 2, &layout()), None, "another pose count");

        // Pose count, bounce count, then probe 0's index and its three marks.
        let marks = 12;
        assert_eq!(&bytes[marks..marks + 3], &[1, 2, 1]);
        let mut nothing = bytes.clone();
        nothing[marks..marks + 3].copy_from_slice(&[0, 2, 0]);
        assert_eq!(decode(&nothing, 5, 3, &layout()), None, "stores nothing");
        let mut base_terminus = bytes.clone();
        base_terminus[marks] = 2;
        assert_eq!(
            decode(&base_terminus, 5, 3, &layout()),
            None,
            "base at a terminus"
        );
        let mut unknown = bytes.clone();
        unknown[marks + 1] = 3;
        assert_eq!(decode(&unknown, 5, 3, &layout()), None, "an unknown mark");
        let mut torn = bytes;
        torn.pop();
        assert_eq!(decode(&torn, 5, 3, &layout()), None, "a torn tile run");
    }
}
