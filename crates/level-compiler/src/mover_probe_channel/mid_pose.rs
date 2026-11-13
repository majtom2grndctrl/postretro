// Mid-pose states: what each mid pose stores or reads from the base at each
// near-field probe, from its own posed gather. A probe the pose encloses
// stores the state that gather gives, as any other probe does: zero bounce
// and the dark depth tile.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};
use rayon::prelude::*;

use super::domain::{clears_bounce_bars, compact_tile, round_trip_error};
use super::gather::DomainProbe;
use super::states::{MidPoseStates, MidState, compact_rgb};
use crate::bake_control::BakeControl;
use crate::sh_bake::pack_octahedral_irradiance_tile;

/// What a mid pose holds for one near-field probe from its posed gather
/// `coefficients`, against the probe's id-34 tile `base` and that tile's
/// round-trip bar: its state, as id 27's compact RGB tile, where it clears
/// both bars against id 34 (the round-trip bar and the perceptual floor, as
/// an affected terminus's must), and the base where it stays within them.
pub(crate) fn classify(
    coefficients: &[f32; 27],
    base: &[OctahedralAtlasTexel],
    bar: f32,
) -> MidState<Vec<u16>> {
    let tile = pack_octahedral_irradiance_tile(
        coefficients,
        true,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    );
    if clears_bounce_bars(&tile, base, bar) {
        MidState::Stored(compact_rgb(&tile))
    } else {
        MidState::Base
    }
}

/// A mid pose from its near-field gather `gathered` (ascending by probe):
/// each probe's bounce by [`classify`] against its id-34 tile in `base`
/// (`slots` maps a dense probe to its compact slot), and its depth tile
/// where some ray met the pose, else its base.
///
/// A probe the pose encloses is classified like every other (Amendment 2,
/// Ruling B): it stores the state its own posed gather gives. Every lifted
/// ray starts in solid and reads black, so its bounce is zero radiance,
/// stored where that leaves its bars against id 34 and the base where id 34
/// is already within them; every ray meets the pose at distance 0, so it
/// stores the dark depth tile. Enclosure decides nothing here: the gather
/// traces it, no rule writes it. Blending such a probe across, as before,
/// lit a probe layer inside a thin mover wall from its lit neighbours and
/// pulsed a band into the room behind it. Termini still read the base
/// ([`super::gather::PosedProbe::changed`]). Once the mover's poses reduce,
/// the first and last poses of each run of enclosing poses take their clear
/// neighbour's value state instead ([`super::edge_copy`]).
pub(crate) fn classify_mid_pose(
    gathered: Vec<DomainProbe>,
    base: &OctahedralShVolumeSection,
    slots: &[u32],
    uncompressed: bool,
    control: &BakeControl,
) -> MidPoseStates {
    let dim = base.tile_dimension as usize;
    let classified: Vec<_> = gathered
        .into_par_iter()
        .map(|probe| {
            let _permit = control.governor().enter();
            let tile = compact_tile(base, slots[probe.probe as usize] as usize);
            let bar = round_trip_error(&tile, dim, uncompressed);
            let bounce = classify(&probe.coefficients, &tile, bar);
            (probe.probe, bounce, depth_state(probe.depth))
        })
        .collect();
    let mut states = MidPoseStates::default();
    for (probe, bounce, depth) in classified {
        states.bounce.push((probe, bounce));
        states.depth.push((probe, depth));
    }
    states
}

/// A depth-only mid pose (`light_bounces` 0) over the domain's probes
/// (ascending): the tiles of those its rays met, the base elsewhere.
pub(crate) fn depth_only_mid_pose(domain: &[u32], met: Vec<(u32, Vec<[u16; 2]>)>) -> MidPoseStates {
    let mut met = met.into_iter().peekable();
    let depth = domain
        .iter()
        .map(|&probe| {
            let tile = met.next_if(|(at, _)| *at == probe).map(|(_, tile)| tile);
            (probe, depth_state(tile))
        })
        .collect();
    MidPoseStates {
        bounce: Vec::new(),
        depth,
    }
}

fn depth_state(tile: Option<Vec<[u16; 2]>>) -> MidState<Vec<[u16; 2]>> {
    tile.map_or(MidState::Base, MidState::Stored)
}

#[cfg(test)]
#[path = "mid_pose_tests.rs"]
mod tests;
