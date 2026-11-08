// Probe depth tiles (id 59): gather distances binned per direction, selected by geometry, capped.
// See: context/lib/build_pipeline.md §PRL section IDs · context/lib/rendering_pipeline.md §4

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use glam::Vec3;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::irradiance_interior_texel_direction;
use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_BORDER, PROBE_DEPTH_TILE_BORDERED_EDGE,
    PROBE_DEPTH_TILE_EDGE, PROBE_DEPTH_TILE_INTERIOR_TEXELS, PROBE_DEPTH_TILE_TEXELS,
    PROBE_DEPTH_TILES_VERSION, ProbeDepthTilesSection, bordered_tile, f16_bits_to_f32,
    interior_texel_of_direction, probe_depth_tile_texture_bytes, sample_tile_moments,
};
use postretro_level_format::sh_volume::OctahedralShProbe;

use super::grid::{ProbeGridLayout, probe_index_to_xyz};

/// A valid probe gets a tile when its tile moves its visibility toward some
/// valid lattice neighbour by more than this. At 8×8 tiles, 0.1 flagged 1.40%
/// of the hallway's valid probes and every closet probe with a blocked valid
/// neighbour.
pub(crate) const TILE_SELECTION_THRESHOLD: f32 = 0.1;

/// Largest finite f16. A tile moment past it would encode +inf, and a
/// bilinear tap weighing +inf by zero reads NaN, which escapes the visibility
/// floor; tile texels saturate here instead.
const F16_MAX: f32 = 65504.0;

// `sh_sample.wgsl`'s Chebyshev constants, mirrored.
const SH_DEPTH_MIN_VARIANCE_M2: f32 = 1.0e-4;
const SH_DEPTH_BIAS_CELL_FRACTION: f32 = 0.05;
const SH_DEPTH_MIN_VISIBILITY: f32 = 0.03;

/// `sh_probe_depth_bias`: a fraction of the smallest cell axis.
pub(crate) fn probe_depth_bias(cell_size: [f32; 3]) -> f32 {
    Vec3::from(cell_size).min_element().max(0.0) * SH_DEPTH_BIAS_CELL_FRACTION
}

/// `sh_corner_depth_visibility` for a valid probe: Chebyshev's bound on
/// `moments` (`E[d]`, `E[d²]`) at `distance`, floored and clamped as the
/// runtime clamps it.
pub(crate) fn chebyshev_visibility([mean, mean2]: [f32; 2], distance: f32, bias: f32) -> f32 {
    let variance = (mean2 - mean * mean).max(SH_DEPTH_MIN_VARIANCE_M2);
    let delta = (distance - mean - bias).max(0.0);
    let visibility = if delta > 0.0 {
        variance / (variance + delta * delta)
    } else {
        1.0
    };
    visibility.clamp(SH_DEPTH_MIN_VISIBILITY, 1.0)
}

/// The cap a bake keeps: the one asked for, at most the reference's
/// [`MAX_PROBE_DEPTH_TILES`].
pub(crate) fn effective_tile_cap(tile_cap: u32) -> u32 {
    tile_cap.min(MAX_PROBE_DEPTH_TILES)
}

/// Fold what decides the tiles beyond the gather's own constants: the tile
/// format and its saturation, the selection threshold, the Chebyshev
/// constants it mirrors and the cap the bake keeps.
pub(crate) fn fold_depth_tile_constants(hasher: &mut blake3::Hasher, tile_cap: u32) {
    hasher.update(b"probe depth tiles\0");
    for word in [
        PROBE_DEPTH_TILES_VERSION,
        PROBE_DEPTH_TILE_EDGE,
        PROBE_DEPTH_TILE_BORDER,
        effective_tile_cap(tile_cap),
    ] {
        hasher.update(&word.to_le_bytes());
    }
    for value in [
        F16_MAX,
        TILE_SELECTION_THRESHOLD,
        SH_DEPTH_MIN_VARIANCE_M2,
        SH_DEPTH_BIAS_CELL_FRACTION,
        SH_DEPTH_MIN_VISIBILITY,
    ] {
        hasher.update(&value.to_le_bytes());
    }
}

/// Host bytes the tiles of a `probes`-probe bake can hold in one
/// representation at the production cap: the largest per-tile form (a ranked
/// candidate) for every tile the cap allows. The bake never holds more than
/// two representations at once.
pub(crate) fn depth_tile_bytes(probes: u64) -> u64 {
    probes.min(u64::from(MAX_PROBE_DEPTH_TILES)) * size_of::<Candidate>() as u64
}

/// The tiles one SH base bake keeps, and how many candidates the cap turned
/// away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProbeDepthTiles {
    pub(crate) section: ProbeDepthTilesSection,
    pub(crate) dropped: u64,
}

impl ProbeDepthTiles {
    /// Heap bytes, by capacity, for the test ledger.
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        (self.section.probe_indices.capacity() * size_of::<u32>()
            + self.section.texels.capacity() * size_of::<[u16; 2]>()) as u64
    }

    /// The bytes the runtime's tile texture allocates for these tiles.
    pub(crate) fn texture_bytes(&self) -> u64 {
        probe_depth_tile_texture_bytes(self.section.tile_count() as u32)
    }

    /// The once-per-build info line, warm or cold, for a bake asked to keep
    /// `tile_cap`.
    pub(crate) fn log(&self, tile_cap: u32) {
        log::info!(
            "[Compiler] Probe depth tiles: {} kept, {} dropped past the {}-tile cap",
            self.section.tile_count(),
            self.dropped,
            effective_tile_cap(tile_cap),
        );
    }

    /// The Build Summary line for a bake asked to keep `tile_cap`.
    pub(crate) fn summary_line(&self, tile_cap: u32) -> String {
        let bytes = self.texture_bytes();
        format!(
            "Probe depth tiles: {} tiles, {} dropped past the {}-tile cap; \
             tile texture {bytes} bytes ({:.2} MiB)",
            self.section.tile_count(),
            self.dropped,
            effective_tile_cap(tile_cap),
            bytes as f64 / (1024.0 * 1024.0),
        )
    }
}

/// Which of the gather's fixed directions feed each interior tile texel.
/// Built once per volume from the one direction set every probe shares. A
/// texel no direction lands in borrows the direction nearest its centre, so
/// a tile has no holes.
pub(crate) struct TileRayTable {
    /// Per interior texel (`x + y·8`), its ray indices, ascending.
    members: Vec<Vec<u16>>,
}

impl TileRayTable {
    pub(crate) fn new(directions: &[Vec3]) -> Self {
        let mut members = vec![Vec::new(); PROBE_DEPTH_TILE_INTERIOR_TEXELS];
        for (ray, dir) in directions.iter().enumerate() {
            members[interior_texel_of_direction(dir.to_array())].push(ray as u16);
        }
        let edge = PROBE_DEPTH_TILE_EDGE as usize;
        for (texel, rays) in members.iter_mut().enumerate() {
            if !rays.is_empty() {
                continue;
            }
            let centre = Vec3::from(irradiance_interior_texel_direction(
                (texel % edge) as u32,
                (texel / edge) as u32,
                PROBE_DEPTH_TILE_BORDERED_EDGE,
                PROBE_DEPTH_TILE_BORDER,
            ));
            // The first ray of greatest alignment, so a tie resolves the
            // same way on every build.
            let mut nearest = 0;
            for (ray, dir) in directions.iter().enumerate() {
                if centre.dot(*dir) > centre.dot(directions[nearest]) {
                    nearest = ray;
                }
            }
            rays.push(nearest as u16);
        }
        Self { members }
    }

    /// One probe's interior tile from its per-ray hit distances (a miss
    /// already reads the far sentinel): each texel's mean distance and mean
    /// squared distance over its rays, summed in ray order, as f16 bits
    /// saturated at [`F16_MAX`]: hits are unbounded, so `E[d²]` overflows
    /// past ~256 m.
    pub(crate) fn interior(
        &self,
        distances: &[f32],
    ) -> [[u16; 2]; PROBE_DEPTH_TILE_INTERIOR_TEXELS] {
        let mut out = [[0u16; 2]; PROBE_DEPTH_TILE_INTERIOR_TEXELS];
        for (texel, rays) in out.iter_mut().zip(&self.members) {
            let (mut sum_d, mut sum_d2) = (0.0f32, 0.0f32);
            for &ray in rays {
                let d = distances[usize::from(ray)];
                sum_d += d;
                sum_d2 += d * d;
            }
            let n = rays.len() as f32;
            *texel = [
                f32_to_f16_bits((sum_d / n).min(F16_MAX)),
                f32_to_f16_bits((sum_d2 / n).min(F16_MAX)),
            ];
        }
        out
    }
}

/// One probe whose tile changes its visibility past the threshold.
struct Candidate {
    /// The largest absolute visibility change toward a valid neighbour.
    score: f32,
    probe: u32,
    tile: [[u16; 2]; PROBE_DEPTH_TILE_TEXELS],
}

impl Candidate {
    /// Rank: higher score first, then lower probe index. Probe indices are
    /// unique, so the order is total and the kept set never depends on the
    /// order threads offer candidates.
    fn rank(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then(self.probe.cmp(&other.probe))
    }
}

// The kept heap's top is its worst-ranked candidate: "greater" is worse.
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank(other)
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

/// Ranks the bake's candidates into at most `cap` tiles while the gather
/// runs. Each probe offers its own tile from inside the parallel gather, so
/// no probe's ray distances outlive it; the kept heap is reserved up front at
/// `min(cap, valid probes)`, so retained memory follows the cap, never the
/// probe count.
pub(super) struct DepthTileSelector<'a> {
    layout: &'a ProbeGridLayout,
    rays: TileRayTable,
    bias: f32,
    cap: usize,
    kept: Mutex<BinaryHeap<Candidate>>,
    candidates: AtomicU64,
}

impl<'a> DepthTileSelector<'a> {
    pub(super) fn new(layout: &'a ProbeGridLayout, directions: &[Vec3], cap: u32) -> Self {
        let cap = effective_tile_cap(cap) as usize;
        let valid = layout.validity.iter().filter(|&&v| v != 0).count();
        Self {
            layout,
            rays: TileRayTable::new(directions),
            bias: probe_depth_bias(layout.cell_size),
            cap,
            kept: Mutex::new(BinaryHeap::with_capacity(cap.min(valid))),
            candidates: AtomicU64::new(0),
        }
    }

    /// Bytes the kept heap reserved, for the test ledger.
    #[cfg(test)]
    pub(super) fn reserved_bytes(&self) -> u64 {
        let kept = self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (kept.capacity() * size_of::<Candidate>()) as u64
    }

    /// Bin valid probe `probe`'s gather distances into its tile and offer it
    /// when the tile moves its visibility toward a valid neighbour past the
    /// threshold. `averaged` is its id-34 record, whose f16 moments the
    /// runtime reads.
    pub(super) fn consider(&self, probe: usize, averaged: &OctahedralShProbe, distances: &[f32]) {
        let tile = bordered_tile(&self.rays.interior(distances));
        let averaged = [
            f16_bits_to_f32(averaged.mean_distance),
            f16_bits_to_f32(averaged.mean_sq_distance),
        ];
        let score = self.visibility_change(probe, averaged, &tile);
        if score > TILE_SELECTION_THRESHOLD {
            self.offer(Candidate {
                score,
                probe: probe as u32,
                tile: tile
                    .try_into()
                    .expect("a bordered tile has PROBE_DEPTH_TILE_TEXELS texels"),
            });
        }
    }

    /// The largest absolute change between averaged and tile visibility over
    /// `probe`'s valid in-grid lattice neighbours, each at its own distance.
    pub(super) fn visibility_change(
        &self,
        probe: usize,
        averaged: [f32; 2],
        tile: &[[u16; 2]],
    ) -> f32 {
        let dims = self.layout.dims;
        let (x, y, z) = probe_index_to_xyz(probe, dims);
        let cell = Vec3::from(self.layout.cell_size);
        let mut largest = 0.0f32;
        for oz in -1i64..=1 {
            for oy in -1i64..=1 {
                for ox in -1i64..=1 {
                    if (ox, oy, oz) == (0, 0, 0) {
                        continue;
                    }
                    let n = [i64::from(x) + ox, i64::from(y) + oy, i64::from(z) + oz];
                    if n.iter()
                        .zip(dims)
                        .any(|(&c, d)| !(0..i64::from(d)).contains(&c))
                    {
                        continue;
                    }
                    let neighbour = n[0] as usize
                        + n[1] as usize * dims[0] as usize
                        + n[2] as usize * dims[0] as usize * dims[1] as usize;
                    if self.layout.validity[neighbour] == 0 {
                        continue;
                    }
                    let offset = Vec3::new(ox as f32, oy as f32, oz as f32) * cell;
                    let distance = offset.length();
                    let before = chebyshev_visibility(averaged, distance, self.bias);
                    let after = chebyshev_visibility(
                        sample_tile_moments(tile, offset.to_array()),
                        distance,
                        self.bias,
                    );
                    largest = largest.max((after - before).abs());
                }
            }
        }
        largest
    }

    fn offer(&self, candidate: Candidate) {
        self.candidates.fetch_add(1, AtomicOrdering::Relaxed);
        if self.cap == 0 {
            return;
        }
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if kept.len() < self.cap {
            kept.push(candidate);
        } else if let Some(mut worst) = kept.peek_mut()
            && candidate.rank(&worst) == Ordering::Less
        {
            *worst = candidate;
        }
    }

    /// The kept tiles in ascending probe order, as id 59 stores them.
    pub(super) fn finish(self) -> ProbeDepthTiles {
        let candidates = self.candidates.into_inner();
        let mut kept = self
            .kept
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .into_vec();
        kept.sort_unstable_by_key(|candidate| candidate.probe);
        let mut texels = Vec::with_capacity(kept.len() * PROBE_DEPTH_TILE_TEXELS);
        for candidate in &kept {
            texels.extend_from_slice(&candidate.tile);
        }
        ProbeDepthTiles {
            section: ProbeDepthTilesSection {
                probe_indices: kept.iter().map(|candidate| candidate.probe).collect(),
                texels,
            },
            dropped: candidates - kept.len() as u64,
        }
    }
}

#[cfg(test)]
#[path = "depth_tiles_tests.rs"]
mod tests;
