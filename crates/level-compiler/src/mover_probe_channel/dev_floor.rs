// TEMPORARY dev-only floors for the Stop 2 sweep (mover-light-flow--probe-channel
// task 2): the bounce perceptual floor and the depth significance floor as env
// parameters, the significance score itself, and a per-probe table the sweep
// reads. With no env var set nothing changes. Plan task 12 deletes the env
// plumbing; the production bake keeps the two rules with owner-set values.
// See: context/plans/in-progress/mover-light-flow--probe-channel/index.md (Bounce half, Closed-pose depth)

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_TEXELS, f16_bits_to_f32, sample_tile_moments,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::PosedBounce;
use crate::sh_bake::{ProbeDepthTiles, probe_depth_bias, visibility_change};

/// Display levels ([`super::domain::display_levels`]): a probe is affected
/// only when its change also exceeds this. Unset, no floor.
pub(crate) const BOUNCE_FLOOR_VAR: &str = "POSTRETRO_DEV_BOUNCE_FLOOR";
/// Id 59's visibility-change score: a probe carries mover depth only when
/// its closed-pose tile's score exceeds this. Unset, no floor.
pub(crate) const DEPTH_FLOOR_VAR: &str = "POSTRETRO_DEV_DEPTH_FLOOR";
/// A path: write every posed probe's measures and id 59's kept scores, one
/// whitespace-separated row each, for the floor sweep.
pub(crate) const PROBE_TABLE_VAR: &str = "POSTRETRO_DEV_PROBE_TABLE";

fn floor(var: &'static str, cell: &'static OnceLock<Option<f32>>) -> Option<f32> {
    *cell.get_or_init(|| std::env::var(var).ok().and_then(|v| v.parse().ok()))
}

/// [`BOUNCE_FLOOR_VAR`], read once.
pub(crate) fn bounce_floor() -> Option<f32> {
    static FLOOR: OnceLock<Option<f32>> = OnceLock::new();
    floor(BOUNCE_FLOOR_VAR, &FLOOR)
}

/// [`DEPTH_FLOOR_VAR`], read once.
pub(crate) fn depth_floor() -> Option<f32> {
    static FLOOR: OnceLock<Option<f32>> = OnceLock::new();
    floor(DEPTH_FLOOR_VAR, &FLOOR)
}

/// Scores probe tiles against what each probe reads without them: its id-59
/// tile where id 59 keeps one, else its averaged id-34 moments.
pub(crate) struct Significance<'a> {
    section: &'a OctahedralShVolumeSection,
    validity: Vec<u8>,
    bias: f32,
    id59: BTreeMap<u32, &'a [[u16; 2]]>,
}

impl<'a> Significance<'a> {
    pub(crate) fn new(section: &'a OctahedralShVolumeSection, tiles: &'a ProbeDepthTiles) -> Self {
        Self {
            section,
            validity: section.probes.iter().map(|p| p.validity).collect(),
            bias: probe_depth_bias(section.cell_size),
            id59: tiles
                .section
                .probe_indices
                .iter()
                .copied()
                .zip(tiles.section.texels.chunks(PROBE_DEPTH_TILE_TEXELS))
                .collect(),
        }
    }

    fn averaged(&self, probe: u32) -> [f32; 2] {
        let record = &self.section.probes[probe as usize];
        [
            f16_bits_to_f32(record.mean_distance),
            f16_bits_to_f32(record.mean_sq_distance),
        ]
    }

    /// Id 59's score for `tile` at `probe` against the mover-absent moments.
    pub(crate) fn score(&self, probe: u32, tile: &[[u16; 2]]) -> f32 {
        let averaged = self.averaged(probe);
        let own = self.id59.get(&probe).copied();
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

    /// Id 59's own kept tiles, rescored against averaged moments as its
    /// selector scored them.
    pub(crate) fn id59_scores(&self) -> Vec<(u32, f32)> {
        self.id59
            .iter()
            .map(|(&probe, tile)| {
                let averaged = self.averaged(probe);
                let score = visibility_change(
                    self.section.grid_dimensions,
                    self.section.cell_size,
                    &self.validity,
                    self.bias,
                    probe as usize,
                    |_| averaged,
                    tile,
                );
                (probe, score)
            })
            .collect()
    }
}

/// Per result, the score of every probe carrying a closed-pose tile.
pub(crate) fn depth_scores(
    results: &[PosedBounce],
    significance: &Significance<'_>,
) -> Vec<BTreeMap<u32, f32>> {
    results
        .iter()
        .map(|posed| {
            posed
                .depth()
                .map(|(probe, tile)| (probe, significance.score(probe, tile)))
                .collect()
        })
        .collect()
}

/// Whether a probe with this score keeps its closed-pose tile.
pub(crate) fn keeps_depth(score: f32) -> bool {
    depth_floor().is_none_or(|floor| score > floor)
}

/// `dims nx ny nz`, then `probe <mover> <terminus> <index> <distance m>
/// <|S^b − id 34|> <bar> <levels> <score or -1>` per posed probe, then
/// `id59 <index> <score>` per id-59 tile.
pub(crate) fn write_table(
    path: &Path,
    section: &OctahedralShVolumeSection,
    results: &[PosedBounce],
    scores: &[BTreeMap<u32, f32>],
    id59: &[(u32, f32)],
) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    let [nx, ny, nz] = section.grid_dimensions;
    writeln!(out, "dims {nx} {ny} {nz}")?;
    for (posed, scores) in results.iter().zip(scores) {
        for probe in &posed.probes {
            writeln!(
                out,
                "probe {} {} {} {} {} {} {} {}",
                posed.mover_id,
                posed.terminus.label(),
                probe.probe,
                probe.distance_m,
                probe.difference,
                probe.bar,
                probe.levels,
                scores.get(&probe.probe).copied().unwrap_or(-1.0),
            )?;
        }
    }
    for (probe, score) in id59 {
        writeln!(out, "id59 {probe} {score}")?;
    }
    out.flush()
}
