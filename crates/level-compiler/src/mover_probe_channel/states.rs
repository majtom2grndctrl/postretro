// One mover's bounce and closed-pose depth states: its two poses' gathers
// reduced to the probes id 57 stores, and their memo encoding.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::BTreeMap;

use postretro_level_format::mover_probe_channel::MOVER_IRRADIANCE_TILE_F16_COUNT;
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::depth::{DEPTH_SIGNIFICANCE_FLOOR, MoverAbsentDepth};
use crate::sh_bake::ProbeGridLayout;

/// One probe whose posed state leaves its id-34 tile's round-trip bar.
pub(crate) struct ChangedProbe {
    pub(crate) probe: u32,
    pub(crate) tile: Vec<OctahedralAtlasTexel>,
    /// Past the perceptual floor too: the probe joins the bounce domain.
    pub(crate) affected: bool,
}

/// What one pose's gather leaves: the probes whose state leaves the
/// round-trip bar, with their tiles, and the depth tile of every probe a
/// gather ray of which met the pose.
#[derive(Default)]
pub(crate) struct PoseStates {
    /// Ascending by probe.
    pub(crate) changed: Vec<ChangedProbe>,
    /// `(dense probe, closed-pose tile)`, ascending.
    pub(crate) depth: Vec<(u32, Vec<[u16; 2]>)>,
}

impl PoseStates {
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        let changed: usize = self
            .changed
            .iter()
            .map(|probe| probe.tile.capacity() * size_of::<OctahedralAtlasTexel>())
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|(_, tile)| tile.capacity() * size_of::<[u16; 2]>())
            .sum();
        (changed
            + depth
            + self.changed.capacity() * size_of::<ChangedProbe>()
            + self.depth.capacity() * size_of::<(u32, Vec<[u16; 2]>)>()) as u64
    }
}

/// One affected probe: its stored `S^b₀` then its stored `S^b₁`, each id
/// 27's compact RGB tile. A terminus whose state is within the round-trip
/// bar of id 34 stores none and composes id 34 (owner, 2026-11-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BounceProbe {
    pub(crate) probe: u32,
    /// Whether each terminus stores its state; at least one does.
    pub(crate) stored: [bool; 2],
    pub(crate) tiles: Vec<u16>,
}

/// One probe carrying the mover's depth: its stored `D₀` then its stored
/// `D₁`, each id 59's bordered tile. A terminus whose pose meets none of the
/// probe's gather rays stores none, and the probe reads its base there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DepthProbe {
    pub(crate) probe: u32,
    /// Whether each terminus stores its tile; at least one does.
    pub(crate) stored: [bool; 2],
    pub(crate) tiles: Vec<[u16; 2]>,
}

/// One mover's states, as id 57 stores them and its memo keeps them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MoverBounceStates {
    pub(crate) mover_id: u32,
    /// Affected probes, ascending.
    pub(crate) bounce: Vec<BounceProbe>,
    /// Probes carrying closed-pose depth, ascending.
    pub(crate) depth: Vec<DepthProbe>,
    #[cfg(test)]
    held: crate::live_bytes::Held,
}

impl MoverBounceStates {
    pub(crate) fn new(mover_id: u32, bounce: Vec<BounceProbe>, depth: Vec<DepthProbe>) -> Self {
        #[cfg_attr(
            not(test),
            expect(unused_mut, reason = "only the test ledger records it")
        )]
        let mut states = Self {
            mover_id,
            bounce,
            depth,
            #[cfg(test)]
            held: crate::live_bytes::Held::default(),
        };
        #[cfg(test)]
        {
            states.held = crate::live_bytes::hold("mover bounce states", states.heap_bytes());
        }
        states
    }

    #[cfg(test)]
    fn heap_bytes(&self) -> u64 {
        let bounce: usize = self
            .bounce
            .iter()
            .map(|probe| probe.tiles.capacity() * size_of::<u16>())
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|probe| probe.tiles.capacity() * size_of::<[u16; 2]>())
            .sum();
        (bounce
            + depth
            + self.bounce.capacity() * size_of::<BounceProbe>()
            + self.depth.capacity() * size_of::<DepthProbe>()) as u64
    }
}

/// A tile's RGB, texel by texel: id 27's compact tile.
fn compact_rgb(tile: &[OctahedralAtlasTexel]) -> impl Iterator<Item = u16> + '_ {
    tile.iter()
        .flat_map(|texel| [texel.rgba[0], texel.rgba[1], texel.rgba[2]])
}

/// Reduce one mover's two poses (by terminus) to its states.
///
/// Bounce: a probe affected at either terminus joins the domain. At each
/// terminus it stores its state when that state leaves the round-trip bar
/// of id 34; within the bar it stores none and composes id 34 there, the
/// texel the frame composes with no section (owner, 2026-11-09).
///
/// Depth: a probe some gather ray of which met the pose at either terminus
/// carries the mover's depth when a closed-pose tile of it scores at least
/// [`DEPTH_SIGNIFICANCE_FLOOR`] against its mover-absent depth. It stores a
/// tile at each terminus whose pose some gather ray of it meets. At a
/// terminus no ray meets, the posed gather bins the mover-absent tile
/// exactly, so it stores none and the probe reads its base there: its id-59
/// tile or its averaged moments.
pub(crate) fn reduce_poses(
    mover_id: u32,
    poses: &[PoseStates; 2],
    absent: &MoverAbsentDepth<'_>,
) -> MoverBounceStates {
    let mut changed: BTreeMap<u32, [Option<&ChangedProbe>; 2]> = BTreeMap::new();
    for (terminus, pose) in poses.iter().enumerate() {
        for probe in &pose.changed {
            changed.entry(probe.probe).or_default()[terminus] = Some(probe);
        }
    }
    let bounce = changed
        .into_iter()
        .filter(|(_, states)| states.iter().flatten().any(|state| state.affected))
        .map(|(probe, states)| {
            let mut tiles = Vec::with_capacity(2 * MOVER_IRRADIANCE_TILE_F16_COUNT);
            for state in states.iter().flatten() {
                tiles.extend(compact_rgb(&state.tile));
            }
            BounceProbe {
                probe,
                stored: states.map(|state| state.is_some()),
                tiles,
            }
        })
        .collect();

    let mut met: BTreeMap<u32, [Option<&[[u16; 2]]>; 2]> = BTreeMap::new();
    for (terminus, pose) in poses.iter().enumerate() {
        for (probe, tile) in &pose.depth {
            met.entry(*probe).or_default()[terminus] = Some(tile);
        }
    }
    let depth = met
        .into_iter()
        .filter(|(probe, tiles)| {
            tiles
                .iter()
                .flatten()
                .any(|tile| absent.score(*probe, tile) >= DEPTH_SIGNIFICANCE_FLOOR)
        })
        .map(|(probe, tiles)| {
            let mut stored = Vec::with_capacity(2 * PROBE_DEPTH_TILE_TEXELS);
            for tile in tiles.iter().flatten() {
                stored.extend_from_slice(tile);
            }
            DepthProbe {
                probe,
                stored: tiles.map(|tile| tile.is_some()),
                tiles: stored,
            }
        })
        .collect();
    MoverBounceStates::new(mover_id, bounce, depth)
}

/// Stored termini as one byte: bit k stores terminus k.
fn stored_bits(stored: [bool; 2]) -> u8 {
    u8::from(stored[0]) | u8::from(stored[1]) << 1
}

/// The stored termini of a well-formed byte: at least one, no other bit.
fn stored_from_bits(bits: u8) -> Option<[bool; 2]> {
    (bits != 0 && bits & !0b11 == 0).then_some([bits & 1 != 0, bits & 2 != 0])
}

/// `u32` bounce count, then per probe `u32` index, `u8` stored termini and
/// 108 little-endian halves per stored terminus; `u32` depth count, then per
/// probe `u32` index, `u8` stored termini and 100 `[f16; 2]` texels per
/// stored terminus. The mover id is the memo key's, so it is not stored.
pub(crate) fn encode(states: &MoverBounceStates) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(states.bounce.len() as u32).to_le_bytes());
    for probe in &states.bounce {
        out.extend_from_slice(&probe.probe.to_le_bytes());
        out.push(stored_bits(probe.stored));
        for half in &probe.tiles {
            out.extend_from_slice(&half.to_le_bytes());
        }
    }
    out.extend_from_slice(&(states.depth.len() as u32).to_le_bytes());
    for probe in &states.depth {
        out.extend_from_slice(&probe.probe.to_le_bytes());
        out.push(stored_bits(probe.stored));
        for texel in &probe.tiles {
            for half in texel {
                out.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    out
}

/// The states of a well-formed payload: probes strictly ascending, each a
/// valid probe of `layout` storing at least one terminus, every tile whole,
/// and no trailing bytes.
pub(crate) fn decode(
    bytes: &[u8],
    mover_id: u32,
    layout: &ProbeGridLayout,
) -> Option<MoverBounceStates> {
    let mut reader = Reader(bytes);
    let valid = |probe: u32| layout.validity.get(probe as usize).is_some_and(|&v| v != 0);
    let count = reader.u32()? as usize;
    let mut bounce: Vec<BounceProbe> = Vec::with_capacity(count.min(bytes.len() / 4));
    for _ in 0..count {
        let probe = reader.u32()?;
        if !valid(probe) || bounce.last().is_some_and(|last| last.probe >= probe) {
            return None;
        }
        let stored = stored_from_bits(reader.u8()?)?;
        let tile_count = stored.iter().filter(|&&s| s).count();
        let tiles = reader
            .take(tile_count * MOVER_IRRADIANCE_TILE_F16_COUNT * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&half| u16::from_le_bytes(half))
            .collect();
        bounce.push(BounceProbe {
            probe,
            stored,
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
        let stored = stored_from_bits(reader.u8()?)?;
        let tile_count = stored.iter().filter(|&&s| s).count();
        let tiles = reader
            .take(tile_count * PROBE_DEPTH_TILE_TEXELS * 4)?
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
            stored,
            tiles,
        });
    }
    reader
        .0
        .is_empty()
        .then(|| MoverBounceStates::new(mover_id, bounce, depth))
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
}

#[cfg(test)]
mod tests {
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
                stored: [true, true],
                tiles: (0..2 * MOVER_IRRADIANCE_TILE_F16_COUNT as u16).collect(),
            },
            BounceProbe {
                probe: 3,
                stored: [true, false],
                tiles: vec![7; MOVER_IRRADIANCE_TILE_F16_COUNT],
            },
        ];
        let depth = vec![DepthProbe {
            probe: 1,
            stored: [false, true],
            tiles: vec![[1, 2]; PROBE_DEPTH_TILE_TEXELS],
        }];
        MoverBounceStates::new(5, bounce, depth)
    }

    // Task 11c: the memo keeps each probe's stored termini and only their
    // tiles, and refuses a probe that stores neither or a torn tile run.
    #[test]
    fn the_memo_round_trips_one_sided_states_and_refuses_a_probe_storing_nothing() {
        let states = states();
        let bytes = encode(&states);
        assert_eq!(decode(&bytes, 5, &layout()), Some(states));

        // Bounce count, then probe 0's index and its stored-termini byte.
        let mut nothing = bytes.clone();
        assert_eq!(nothing[8], 0b11);
        nothing[8] = 0;
        assert_eq!(decode(&nothing, 5, &layout()), None);
        let mut reserved = bytes.clone();
        reserved[8] = 0b111;
        assert_eq!(decode(&reserved, 5, &layout()), None);
        let mut torn = bytes;
        torn.pop();
        assert_eq!(decode(&torn, 5, &layout()), None);
    }
}
