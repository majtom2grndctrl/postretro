// Closed-pose depth: which probes carry a mover's depth tiles, and the tile
// range they reserve ahead of id 59's own tiles.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel) · §PRL section IDs

use std::collections::BTreeMap;

use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection, f16_bits_to_f32, sample_tile_moments,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use thiserror::Error;

use crate::sh_bake::{ProbeDepthTiles, effective_tile_cap, probe_depth_bias, visibility_change};

/// The significance floor (owner, Stop 2, 2026-11-08): a probe carries a
/// mover's closed-pose depth only when id 59's own visibility-change score
/// of a closed-pose tile, against the probe's mover-absent moments, is at
/// least this. The largest round value that keeps every probe the static-door
/// reference closet tiles at its door (their scores run 0.065–0.97). The
/// score picks a representation; it never gates a probe's inclusion in the
/// blend and never ships.
pub(crate) const DEPTH_SIGNIFICANCE_FLOOR: f32 = 0.05;

/// Scores a probe's tile against what the probe reads with every mover
/// absent: its id-59 tile where id 59 keeps one, else its averaged id-34
/// moments. Id 59 here is the bake's own selection, before mover tiles
/// reserve their range, so the score never depends on another mover.
pub(crate) struct MoverAbsentDepth<'a> {
    section: &'a OctahedralShVolumeSection,
    validity: Vec<u8>,
    bias: f32,
    tiles: BTreeMap<u32, &'a [[u16; 2]]>,
}

impl<'a> MoverAbsentDepth<'a> {
    pub(crate) fn new(section: &'a OctahedralShVolumeSection, id59: &'a ProbeDepthTiles) -> Self {
        Self {
            section,
            validity: section.probes.iter().map(|probe| probe.validity).collect(),
            bias: probe_depth_bias(section.cell_size),
            tiles: id59
                .section
                .probe_indices
                .iter()
                .copied()
                .zip(id59.section.texels.chunks(PROBE_DEPTH_TILE_TEXELS))
                .collect(),
        }
    }

    fn averaged(&self, probe: u32) -> [f32; 2] {
        averaged_moments(self.section, probe)
    }

    /// Id 59's score for `tile` at `probe`: the largest visibility change it
    /// makes toward a valid neighbour against the mover-absent moments.
    pub(crate) fn score(&self, probe: u32, tile: &[[u16; 2]]) -> f32 {
        let averaged = self.averaged(probe);
        let own = self.tiles.get(&probe).copied();
        visibility_change(
            self.section.grid_dimensions,
            self.section.cell_size,
            &self.validity,
            self.bias,
            probe as usize,
            |offset| own.map_or(averaged, |own| sample_tile_moments(own, offset.to_array())),
            tile,
        )
    }
}

/// A valid probe's averaged id-34 depth moments, as the runtime reads them.
fn averaged_moments(section: &OctahedralShVolumeSection, probe: u32) -> [f32; 2] {
    let record = &section.probes[probe as usize];
    [
        f16_bits_to_f32(record.mean_distance),
        f16_bits_to_f32(record.mean_sq_distance),
    ]
}

/// Live tiles past the tile reference's range: the build refuses rather than
/// drop a mover tile.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "MoverProbeChannel (id 57): {live} probes carry closed-pose depth, past the {cap}-tile \
     probe depth tile range; no mover tile is dropped. Lower the movers' reach into probe \
     depth (fewer movers, or movers meeting fewer probes' gather rays)"
)]
pub(crate) struct MoverTileRangeExceeded {
    pub(crate) live: usize,
    pub(crate) cap: u32,
}

/// What reserving the mover tiles left of id 59.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DepthReservation {
    /// One live tile per probe carrying mover depth.
    pub(crate) live: usize,
    /// The cap id 59 keeps after the live tiles: the range less `live`.
    pub(crate) shrunk_cap: u32,
    /// Id 59 tiles the reservation dropped, on top of the bake's own drops.
    pub(crate) trimmed: u64,
}

/// Reserve `live` tiles of the range `cap` for mover depth, ahead of id 59:
/// id 59 keeps at most `cap − live` tiles, dropping its lowest-ranked as
/// its own selection ranks them (score descending, then probe index
/// ascending), the scores recomputed against the probes' averaged id-34
/// moments since the cached tiles keep none. So id 59 keeps exactly what a
/// selection at the shrunk cap keeps. Live tiles alone past the range are a
/// named error, never a dropped mover tile.
pub(crate) fn reserve_mover_tiles(
    tiles: &mut ProbeDepthTiles,
    base: &OctahedralShVolumeSection,
    live: usize,
    cap: u32,
) -> Result<DepthReservation, MoverTileRangeExceeded> {
    let cap = effective_tile_cap(cap);
    let shrunk_cap = u32::try_from(live)
        .ok()
        .and_then(|live| cap.checked_sub(live))
        .ok_or(MoverTileRangeExceeded { live, cap })?;
    let kept = tiles.section.tile_count();
    if kept <= shrunk_cap as usize {
        return Ok(DepthReservation {
            live,
            shrunk_cap,
            trimmed: 0,
        });
    }
    let validity: Vec<u8> = base.probes.iter().map(|probe| probe.validity).collect();
    let bias = probe_depth_bias(base.cell_size);
    let section = &tiles.section;
    let mut ranked: Vec<(f32, u32, usize)> = section
        .probe_indices
        .iter()
        .enumerate()
        .map(|(index, &probe)| {
            let averaged = averaged_moments(base, probe);
            let score = visibility_change(
                base.grid_dimensions,
                base.cell_size,
                &validity,
                bias,
                probe as usize,
                |_| averaged,
                section.tile(index),
            );
            (score, probe, index)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked.truncate(shrunk_cap as usize);
    ranked.sort_unstable_by_key(|&(_, probe, _)| probe);
    let mut texels = Vec::with_capacity(ranked.len() * PROBE_DEPTH_TILE_TEXELS);
    for &(_, _, index) in &ranked {
        texels.extend_from_slice(section.tile(index));
    }
    let trimmed = (kept - ranked.len()) as u64;
    tiles.section = ProbeDepthTilesSection {
        probe_indices: ranked.iter().map(|&(_, probe, _)| probe).collect(),
        texels,
    };
    tiles.dropped += trimmed;
    Ok(DepthReservation {
        live,
        shrunk_cap,
        trimmed,
    })
}

#[cfg(test)]
#[path = "depth_tests.rs"]
mod tests;
