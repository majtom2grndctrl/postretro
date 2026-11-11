// The per-pose memo's payload: one pose's raw states, and the checks a
// payload must pass before a build trusts it.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::mover_probe_channel::MOVER_IRRADIANCE_TILE_F16_COUNT;
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

use super::pose_memo::PoseKind;
use super::states::{ChangedProbe, MidPoseStates, MidState, PoseState, PoseStates};
use crate::sh_bake::ProbeGridLayout;

const TERMINUS: u8 = 0;
const MID: u8 = 1;
const STORED: u8 = 1;
const BASE: u8 = 2;

/// A kind byte (0 terminus, 1 mid pose), then two probe lists, each a `u32`
/// count and its probes ascending by `u32` index. A terminus lists its
/// changed probes, each with an affected byte (0 or 1) and 108 little-endian
/// halves, then the probes its rays met, each with 100 `[f16; 2]` texels. A
/// mid pose lists its bounce probes, each with a state byte (1 stored, 2
/// base) and, stored, 108 halves, then every domain probe's depth, each with
/// a state byte and, stored, 100 texels.
pub(crate) fn encode(state: &PoseState) -> Vec<u8> {
    let mut out = Vec::new();
    match state {
        PoseState::Terminus(states) => {
            out.push(TERMINUS);
            out.extend_from_slice(&(states.changed.len() as u32).to_le_bytes());
            for probe in &states.changed {
                out.extend_from_slice(&probe.probe.to_le_bytes());
                out.push(u8::from(probe.affected));
                put_halves(&mut out, &probe.tile);
            }
            out.extend_from_slice(&(states.depth.len() as u32).to_le_bytes());
            for (probe, tile) in &states.depth {
                out.extend_from_slice(&probe.to_le_bytes());
                put_depth(&mut out, tile);
            }
        }
        PoseState::Mid(states) => {
            out.push(MID);
            out.extend_from_slice(&(states.bounce.len() as u32).to_le_bytes());
            for (probe, state) in &states.bounce {
                out.extend_from_slice(&probe.to_le_bytes());
                put_mid(&mut out, state, put_halves);
            }
            out.extend_from_slice(&(states.depth.len() as u32).to_le_bytes());
            for (probe, state) in &states.depth {
                out.extend_from_slice(&probe.to_le_bytes());
                put_mid(&mut out, state, put_depth);
            }
        }
    }
    out
}

fn put_halves(out: &mut Vec<u8>, tile: &[u16]) {
    for half in tile {
        out.extend_from_slice(&half.to_le_bytes());
    }
}

fn put_depth(out: &mut Vec<u8>, tile: &[[u16; 2]]) {
    for texel in tile {
        for half in texel {
            out.extend_from_slice(&half.to_le_bytes());
        }
    }
}

fn put_mid<T>(out: &mut Vec<u8>, state: &MidState<Vec<T>>, put: fn(&mut Vec<u8>, &[T])) {
    match state {
        MidState::Stored(tile) => {
            out.push(STORED);
            put(out, tile);
        }
        MidState::Base => out.push(BASE),
    }
}

/// The states of a well-formed payload of `kind`: every probe a valid probe
/// of `layout`, each list strictly ascending, every tile whole and no
/// trailing bytes. A terminus's affected bytes are 0 or 1. A mid pose's
/// state bytes are known, its bounce probes lie in `domain` (its near-field
/// probes, ascending) and its depth lists exactly `domain`, as its bake
/// does.
pub(crate) fn decode(
    bytes: &[u8],
    kind: PoseKind,
    layout: &ProbeGridLayout,
    domain: &[u32],
) -> Option<PoseState> {
    let mut reader = Reader(bytes);
    let valid = |probe: u32| layout.validity.get(probe as usize).is_some_and(|&v| v != 0);
    let state = match (reader.u8()?, kind) {
        (TERMINUS, PoseKind::Terminus) => {
            let changed = reader.list(&valid, |reader, probe| {
                let affected = match reader.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                Some(ChangedProbe {
                    probe,
                    tile: reader.halves()?,
                    affected,
                })
            })?;
            let depth = reader.list(&valid, |reader, probe| Some((probe, reader.depth()?)))?;
            PoseState::Terminus(PoseStates { changed, depth })
        }
        (MID, PoseKind::Mid) => {
            let in_domain = |probe: u32| valid(probe) && domain.binary_search(&probe).is_ok();
            let bounce = reader.list(&in_domain, |reader, probe| {
                Some((probe, reader.mid(Reader::halves)?))
            })?;
            let depth = reader.list(&in_domain, |reader, probe| {
                Some((probe, reader.mid(Reader::depth)?))
            })?;
            if !depth
                .iter()
                .map(|(probe, _)| *probe)
                .eq(domain.iter().copied())
            {
                return None;
            }
            PoseState::Mid(MidPoseStates { bounce, depth })
        }
        _ => return None,
    };
    reader.0.is_empty().then_some(state)
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

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A `u32` count, then that many items, each a probe index `admits`
    /// takes, strictly ascending, followed by what `item` reads.
    fn list<T>(
        &mut self,
        admits: &dyn Fn(u32) -> bool,
        mut item: impl FnMut(&mut Self, u32) -> Option<T>,
    ) -> Option<Vec<T>> {
        let count = self.u32()? as usize;
        let mut items = Vec::with_capacity(count.min(self.0.len() / 4));
        let mut last = None;
        for _ in 0..count {
            let probe = self.u32()?;
            if !admits(probe) || last.is_some_and(|last| last >= probe) {
                return None;
            }
            last = Some(probe);
            items.push(item(self, probe)?);
        }
        Some(items)
    }

    /// One compact bounce tile: 108 halves.
    fn halves(&mut self) -> Option<Vec<u16>> {
        Some(
            self.take(MOVER_IRRADIANCE_TILE_F16_COUNT * 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&half| u16::from_le_bytes(half))
                .collect(),
        )
    }

    /// One bordered depth tile: 100 `[f16; 2]` texels.
    fn depth(&mut self) -> Option<Vec<[u16; 2]>> {
        Some(
            self.take(PROBE_DEPTH_TILE_TEXELS * 4)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|t| {
                    [
                        u16::from_le_bytes([t[0], t[1]]),
                        u16::from_le_bytes([t[2], t[3]]),
                    ]
                })
                .collect(),
        )
    }

    fn mid<T>(&mut self, tile: fn(&mut Self) -> Option<T>) -> Option<MidState<T>> {
        match self.u8()? {
            STORED => tile(self).map(MidState::Stored),
            BASE => Some(MidState::Base),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;

    fn layout() -> ProbeGridLayout {
        ProbeGridLayout {
            world_min: DVec3::ZERO,
            dims: [5, 1, 1],
            cell_size: [1.0; 3],
            far_sentinel: 100.0,
            probe_positions: vec![DVec3::ZERO; 5],
            validity: vec![1, 1, 0, 1, 1],
        }
    }

    fn bounce_tile(seed: u16) -> Vec<u16> {
        (0..MOVER_IRRADIANCE_TILE_F16_COUNT as u16)
            .map(|half| half ^ seed)
            .collect()
    }

    fn depth_tile(seed: u16) -> Vec<[u16; 2]> {
        vec![[seed, seed + 1]; PROBE_DEPTH_TILE_TEXELS]
    }

    fn terminus() -> PoseState {
        PoseState::Terminus(PoseStates {
            changed: vec![
                ChangedProbe {
                    probe: 0,
                    tile: bounce_tile(3),
                    affected: true,
                },
                ChangedProbe {
                    probe: 4,
                    tile: bounce_tile(9),
                    affected: false,
                },
            ],
            depth: vec![(1, depth_tile(5))],
        })
    }

    /// Domain probes 1, 3 and 4.
    const DOMAIN: [u32; 3] = [1, 3, 4];

    fn mid() -> PoseState {
        PoseState::Mid(MidPoseStates {
            bounce: vec![(1, MidState::Stored(bounce_tile(7))), (4, MidState::Base)],
            depth: vec![
                (1, MidState::Base),
                (3, MidState::Stored(depth_tile(2))),
                (4, MidState::Base),
            ],
        })
    }

    // Each kind round-trips bit for bit, and a payload of the other kind, a
    // terminus probe that is not valid, an unknown affected or state byte, a
    // mid-pose probe outside its domain, a mid pose whose depth misses a
    // domain probe, or a torn tile is refused.
    #[test]
    fn a_pose_payload_round_trips_each_kind_and_refuses_malformed_ones() {
        let layout = layout();
        let decode = |bytes: &[u8], kind| decode(bytes, kind, &layout, &DOMAIN);
        let terminus_bytes = encode(&terminus());
        assert_eq!(
            decode(&terminus_bytes, PoseKind::Terminus),
            Some(terminus())
        );
        assert_eq!(decode(&terminus_bytes, PoseKind::Mid), None, "other kind");
        let mid_bytes = encode(&mid());
        assert_eq!(decode(&mid_bytes, PoseKind::Mid), Some(mid()));
        assert_eq!(decode(&mid_bytes, PoseKind::Terminus), None, "other kind");

        // Kind, changed count, then probe 0's index and its affected byte.
        let affected = 1 + 4 + 4;
        assert_eq!(terminus_bytes[affected], 1);
        let mut unknown = terminus_bytes.clone();
        unknown[affected] = 2;
        assert_eq!(decode(&unknown, PoseKind::Terminus), None, "affected byte");
        let mut invalid = terminus_bytes.clone();
        invalid[5..9].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(decode(&invalid, PoseKind::Terminus), None, "invalid probe");
        let mut torn = terminus_bytes;
        torn.pop();
        assert_eq!(decode(&torn, PoseKind::Terminus), None, "a torn tile");

        // Kind, bounce count, then probe 1's index and its state byte.
        let state = 1 + 4 + 4;
        assert_eq!(mid_bytes[state], STORED);
        let mut unknown = mid_bytes.clone();
        unknown[state] = 3;
        assert_eq!(decode(&unknown, PoseKind::Mid), None, "state byte");
        let mut outside = mid_bytes.clone();
        outside[5..9].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode(&outside, PoseKind::Mid), None, "outside its domain");
        assert_eq!(
            super::decode(&mid_bytes, PoseKind::Mid, &layout, &[0, 1, 3, 4]),
            None,
            "a depth list short of the domain"
        );
        let mut torn = mid_bytes;
        torn.pop();
        assert_eq!(decode(&torn, PoseKind::Mid), None, "a torn tile");
    }
}
