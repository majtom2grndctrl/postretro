// Mid-pose states: the termini's later shares over the near-field domain,
// and what each mid pose stores, reads from the base or blends across.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use std::collections::BTreeMap;

use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};
use rayon::prelude::*;

use super::domain::{
    BOUNCE_FLOOR_LEVELS, compact_tile, display_levels, max_abs_difference, round_trip_error,
};
use super::gather::DomainProbe;
use super::states::{MidPoseStates, MidState, PoseStates};
use crate::bake_control::BakeControl;
use crate::bc6h::f16_bits_to_f32;
use crate::sh_bake::{octahedral_irradiance_tile_rgb, pack_rgb_tile};

/// A bordered irradiance tile in `f32` RGB, texel by texel as id 34 lays
/// its tiles out.
pub(crate) type RgbTile = Vec<[f32; 3]>;

/// A gather's SH coefficients as a bordered `f32` tile, before any f16.
pub(crate) fn rgb_tile(coefficients: &[f32; 27]) -> RgbTile {
    octahedral_irradiance_tile_rgb(
        coefficients,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    )
}

/// A stored f16 tile's RGB as `f32`.
fn decoded(tile: &[OctahedralAtlasTexel]) -> RgbTile {
    tile.iter()
        .map(|texel| [0, 1, 2].map(|c| f16_bits_to_f32(texel.rgba[c])))
        .collect()
}

/// A terminus's later share at one probe: its state there, as compose reads
/// it at that terminus (its stored tile, or id 34 where it stores none),
/// less its posed bounce one gathered there. The gather is linear in patch
/// radiance along directions that never depend on radiance, so the share is
/// exactly the gather of the terminus's later bounces, up to `f32` and the
/// state's f16.
pub(crate) fn later_share(state: &[OctahedralAtlasTexel], bounce_one: &[[f32; 3]]) -> RgbTile {
    decoded(state)
        .into_iter()
        .zip(bounce_one)
        .map(|(state, one)| [0, 1, 2].map(|c| state[c] - one[c]))
        .collect()
}

/// The termini's later shares blended to path fraction `t`: the lerp where
/// both lend one, the lending one alone where one encloses the probe, and
/// none where both do. A blend of absolute static-light states by mover
/// position, never driven by a light.
pub(crate) fn blended_share(
    first: Option<&RgbTile>,
    last: Option<&RgbTile>,
    t: f32,
) -> Option<RgbTile> {
    match (first, last) {
        (Some(first), Some(last)) => Some(
            first
                .iter()
                .zip(last)
                .map(|(a, b)| [0, 1, 2].map(|c| (1.0 - t) * a[c] + t * b[c]))
                .collect(),
        ),
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (None, None) => None,
    }
}

/// One near-field probe: its id-34 tile, that tile's round-trip bar, and
/// each terminus's later share there, `None` where that terminus encloses
/// the probe and so lends none.
pub(crate) struct ProbeShares {
    pub(crate) probe: u32,
    pub(crate) base: Vec<OctahedralAtlasTexel>,
    pub(crate) bar: f32,
    pub(crate) shares: [Option<RgbTile>; 2],
}

impl ProbeShares {
    /// The shares at one probe from each terminus's state there (`None`
    /// where it stores none, so it reads `base`) and its posed bounce one.
    pub(crate) fn new(
        probe: u32,
        base: Vec<OctahedralAtlasTexel>,
        bar: f32,
        states: [Option<&[OctahedralAtlasTexel]>; 2],
        bounce_one: [&DomainProbe; 2],
    ) -> Self {
        let shares = [0, 1].map(|k| {
            (!bounce_one[k].enclosed).then(|| {
                later_share(
                    states[k].unwrap_or(&base),
                    &rgb_tile(&bounce_one[k].coefficients),
                )
            })
        });
        Self {
            probe,
            base,
            bar,
            shares,
        }
    }

    #[cfg(test)]
    fn heap_bytes(&self) -> u64 {
        (self.base.capacity() * size_of::<OctahedralAtlasTexel>()
            + self
                .shares
                .iter()
                .flatten()
                .map(|share| share.capacity() * size_of::<[f32; 3]>())
                .sum::<usize>()) as u64
    }

    /// What a mid pose at path fraction `t` holds for this probe, from its
    /// own posed bounce one `bounce_one` there: its state, bounce one plus
    /// the blended shares, where that clears both of the probe's bars
    /// against id 34 (the round-trip bar and the perceptual floor, as an
    /// affected terminus's must); the base where the pose encloses the probe
    /// or its state stays within them; nothing, so it blends across, where
    /// neither terminus lends a share.
    pub(crate) fn classify(
        &self,
        bounce_one: &[f32; 27],
        enclosed: bool,
        t: f32,
    ) -> Option<MidState<Vec<OctahedralAtlasTexel>>> {
        if enclosed {
            // DIAGNOSTIC (task 5b): remove before landing.
            match diag_mid_enclosed() {
                Some("blend") => return None,
                Some("dark") => {
                    return Some(MidState::Stored(pack_rgb_tile(&vec![
                        [0.0; 3];
                        self.base.len()
                    ])));
                }
                _ => {}
            }
            return Some(MidState::Base);
        }
        let share = blended_share(self.shares[0].as_ref(), self.shares[1].as_ref(), t)?;
        let state: RgbTile = rgb_tile(bounce_one)
            .into_iter()
            .zip(&share)
            .map(|(one, later)| [0, 1, 2].map(|c| one[c] + later[c]))
            .collect();
        let tile = pack_rgb_tile(&state);
        let clears = max_abs_difference(&tile, &self.base) > self.bar
            && display_levels(&tile, &self.base) > BOUNCE_FLOOR_LEVELS;
        Some(if clears {
            MidState::Stored(tile)
        } else {
            // DIAGNOSTIC (task 5b): remove before landing.
            if diag_mid_base() == Some("unmarked") {
                return None;
            }
            MidState::Base
        })
    }
}

// DIAGNOSTIC (task 5b): remove before landing. Experiment switches read once
// per process: `POSTRETRO_DIAG_MID_ENCLOSED=blend|dark` (an enclosed mid pose
// blends across, or stores its posed dark state, instead of reading the base
// with q = 255), `POSTRETRO_DIAG_MID_BASE=unmarked` (a within-bars mid pose
// blends across instead of reading the base).
pub(crate) fn diag_mid_enclosed() -> Option<&'static str> {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VALUE
        .get_or_init(|| std::env::var("POSTRETRO_DIAG_MID_ENCLOSED").ok())
        .as_deref()
}

// DIAGNOSTIC (task 5b): remove before landing.
pub(crate) fn diag_mid_base() -> Option<&'static str> {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VALUE
        .get_or_init(|| std::env::var("POSTRETRO_DIAG_MID_BASE").ok())
        .as_deref()
}

/// Every near-field probe's shares, in the domain's ascending order.
pub(crate) struct DomainShares {
    pub(crate) probes: Vec<ProbeShares>,
    #[cfg(test)]
    _held: crate::live_bytes::Held,
}

impl DomainShares {
    /// The shares over the domain's probes from both termini: each
    /// terminus's states as the termini alone decided them (`stores`, its
    /// full gather's `termini`) and its posed bounce one gathered over the
    /// domain (`bounce_one`, aligned with `domain`). `slots` maps a dense
    /// probe to id 34's compact slot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        domain: &[u32],
        termini: &[PoseStates; 2],
        stores: &BTreeMap<u32, [bool; 2]>,
        bounce_one: &[Vec<DomainProbe>; 2],
        base: &OctahedralShVolumeSection,
        slots: &[u32],
        uncompressed: bool,
        control: &BakeControl,
    ) -> Self {
        let dim = base.tile_dimension as usize;
        let probes: Vec<ProbeShares> = domain
            .par_iter()
            .enumerate()
            .map(|(at, &probe)| {
                let _permit = control.governor().enter();
                let tile = compact_tile(base, slots[probe as usize] as usize);
                let bar = round_trip_error(&tile, dim, uncompressed);
                let stored = stores.get(&probe).copied().unwrap_or([false; 2]);
                let states = [0, 1].map(|k| {
                    stored[k].then(|| {
                        termini[k]
                            .changed(probe)
                            .expect("a stored terminus changed the probe")
                            .tile
                            .as_slice()
                    })
                });
                debug_assert!(
                    bounce_one
                        .iter()
                        .all(|gathered| gathered[at].probe == probe)
                );
                ProbeShares::new(
                    probe,
                    tile,
                    bar,
                    states,
                    [&bounce_one[0][at], &bounce_one[1][at]],
                )
            })
            .collect();
        Self {
            #[cfg(test)]
            _held: crate::live_bytes::hold(
                "mover later shares",
                (probes.capacity() * size_of::<ProbeShares>()) as u64
                    + probes.iter().map(ProbeShares::heap_bytes).sum::<u64>(),
            ),
            probes,
        }
    }
}

/// A mid pose at path fraction `t` from its near-field gather `gathered`
/// (aligned with the domain), the shares blended to `t`: each probe's
/// bounce by [`ProbeShares::classify`], and its depth tile where some ray
/// met the pose, else its base. A probe the pose encloses meets every ray
/// at distance 0, so it stores the dark tile.
pub(crate) fn classify_mid_pose(
    shares: &DomainShares,
    gathered: Vec<DomainProbe>,
    t: f32,
    control: &BakeControl,
) -> MidPoseStates {
    debug_assert_eq!(shares.probes.len(), gathered.len());
    let classified: Vec<_> = shares
        .probes
        .par_iter()
        .zip(gathered)
        .map(|(shares, probe)| {
            let _permit = control.governor().enter();
            debug_assert_eq!(shares.probe, probe.probe);
            let bounce = shares.classify(&probe.coefficients, probe.enclosed, t);
            (probe.probe, bounce, depth_state(probe.depth))
        })
        .collect();
    let mut states = MidPoseStates::default();
    for (probe, bounce, depth) in classified {
        if let Some(bounce) = bounce {
            states.bounce.push((probe, bounce));
        }
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
