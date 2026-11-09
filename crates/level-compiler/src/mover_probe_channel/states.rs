// One mover's bounce and closed-pose depth states: its two poses' gathers
// reduced to the probes id 57 stores, and their memo encoding.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::BTreeMap;

use glam::Vec3;
use postretro_level_format::mover_probe_channel::MOVER_IRRADIANCE_TILE_F16_COUNT;
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};

use super::depth::{DEPTH_SIGNIFICANCE_FLOOR, MoverAbsentDepth};
use super::domain::compact_tile;
use super::gather::static_depth_tile;
use crate::sh_bake::{ProbeGridLayout, RaytracingCtx, TileRayTable};

/// What one pose's gather leaves: its affected probes' tiles and the depth
/// tile of every probe a gather ray of which met the pose.
#[derive(Default)]
pub(crate) struct PoseStates {
    /// `(dense probe, S^b tile)`, ascending.
    pub(crate) affected: Vec<(u32, Vec<OctahedralAtlasTexel>)>,
    /// `(dense probe, closed-pose tile)`, ascending.
    pub(crate) depth: Vec<(u32, Vec<[u16; 2]>)>,
}

impl PoseStates {
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        let affected: usize = self
            .affected
            .iter()
            .map(|(_, tile)| tile.capacity() * size_of::<OctahedralAtlasTexel>())
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|(_, tile)| tile.capacity() * size_of::<[u16; 2]>())
            .sum();
        (affected
            + depth
            + self.affected.capacity() * size_of::<(u32, Vec<OctahedralAtlasTexel>)>()
            + self.depth.capacity() * size_of::<(u32, Vec<[u16; 2]>)>()) as u64
    }
}

/// One affected probe: `S^b₀` then `S^b₁`, each id 27's compact RGB tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BounceProbe {
    pub(crate) probe: u32,
    pub(crate) tiles: Vec<u16>,
}

/// One probe carrying the mover's depth: `D₀` then `D₁`, each id 59's
/// bordered tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DepthProbe {
    pub(crate) probe: u32,
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

/// What the reduction reads beside the poses.
pub(crate) struct ReduceInputs<'a> {
    pub(crate) ctx: &'a RaytracingCtx<'a>,
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) base: &'a OctahedralShVolumeSection,
    /// Each dense probe's compact slot in `base`.
    pub(crate) slots: &'a [u32],
    pub(crate) absent: &'a MoverAbsentDepth<'a>,
    pub(crate) rays: &'a TileRayTable,
    pub(crate) directions: &'a [Vec3],
}

/// A tile's RGB, texel by texel: id 27's compact tile.
fn compact_rgb(tile: &[OctahedralAtlasTexel]) -> impl Iterator<Item = u16> + '_ {
    tile.iter()
        .flat_map(|texel| [texel.rgba[0], texel.rgba[1], texel.rgba[2]])
}

/// Reduce one mover's two poses (by terminus) to its states.
///
/// Bounce: a probe affected at either terminus stores both termini's tiles;
/// at a terminus where it is not affected it stores id 34's tile, as an
/// unaffected probe composes there.
///
/// Depth: a probe some gather ray of which met the pose at either terminus
/// carries the mover's depth when a closed-pose tile of it scores at least
/// [`DEPTH_SIGNIFICANCE_FLOOR`] against its mover-absent moments. At a
/// terminus where no ray met the pose its tile is the static gather's, which
/// the posed gather would bin there too.
pub(crate) fn reduce_poses(
    mover_id: u32,
    poses: &[PoseStates; 2],
    inputs: &ReduceInputs<'_>,
) -> MoverBounceStates {
    let mut bounce: BTreeMap<u32, [Option<&[OctahedralAtlasTexel]>; 2]> = BTreeMap::new();
    for (terminus, pose) in poses.iter().enumerate() {
        for (probe, tile) in &pose.affected {
            bounce.entry(*probe).or_default()[terminus] = Some(tile);
        }
    }
    let bounce = bounce
        .into_iter()
        .map(|(probe, tiles)| {
            let base = compact_tile(inputs.base, inputs.slots[probe as usize] as usize);
            let mut stored = Vec::with_capacity(2 * MOVER_IRRADIANCE_TILE_F16_COUNT);
            for tile in tiles {
                stored.extend(compact_rgb(tile.unwrap_or(&base)));
            }
            debug_assert_eq!(stored.len(), 2 * MOVER_IRRADIANCE_TILE_F16_COUNT);
            BounceProbe {
                probe,
                tiles: stored,
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
                .any(|tile| inputs.absent.score(*probe, tile) >= DEPTH_SIGNIFICANCE_FLOOR)
        })
        .map(|(probe, tiles)| {
            let mut stored = Vec::with_capacity(2 * PROBE_DEPTH_TILE_TEXELS);
            for tile in tiles {
                match tile {
                    Some(tile) => stored.extend_from_slice(tile),
                    None => stored.extend(static_depth_tile(
                        inputs.ctx,
                        inputs.layout,
                        inputs.rays,
                        inputs.directions,
                        probe,
                    )),
                }
            }
            DepthProbe {
                probe,
                tiles: stored,
            }
        })
        .collect();
    MoverBounceStates::new(mover_id, bounce, depth)
}

/// `u32` bounce count, then per probe `u32` index and 216 little-endian
/// halves; `u32` depth count, then per probe `u32` index and 200 `[f16; 2]`
/// texels. The mover id is the memo key's, so it is not stored.
pub(crate) fn encode(states: &MoverBounceStates) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(states.bounce.len() as u32).to_le_bytes());
    for probe in &states.bounce {
        out.extend_from_slice(&probe.probe.to_le_bytes());
        for half in &probe.tiles {
            out.extend_from_slice(&half.to_le_bytes());
        }
    }
    out.extend_from_slice(&(states.depth.len() as u32).to_le_bytes());
    for probe in &states.depth {
        out.extend_from_slice(&probe.probe.to_le_bytes());
        for texel in &probe.tiles {
            for half in texel {
                out.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    out
}

/// The states of a well-formed payload: probes strictly ascending, each a
/// valid probe of `layout`, every tile whole, and no trailing bytes.
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
        let tiles = reader
            .take(2 * MOVER_IRRADIANCE_TILE_F16_COUNT * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&half| u16::from_le_bytes(half))
            .collect();
        bounce.push(BounceProbe { probe, tiles });
    }
    let count = reader.u32()? as usize;
    let mut depth: Vec<DepthProbe> = Vec::with_capacity(count.min(bytes.len() / 4));
    for _ in 0..count {
        let probe = reader.u32()?;
        if !valid(probe) || depth.last().is_some_and(|last| last.probe >= probe) {
            return None;
        }
        let tiles = reader
            .take(2 * PROBE_DEPTH_TILE_TEXELS * 4)?
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
        depth.push(DepthProbe { probe, tiles });
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

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}
