//! Output-preserving SH coarsenability analysis pass (spike, measurement only).
//!
//! Governing intent: `context/research/archived-plans/lighting-scale--adaptive-base-probe-density`
//! (design intent ONLY — its surface-distance classifier and seam proxy are
//! ABANDONED). This module classifies coarsenability from **composed receiver
//! error**, measures seams as **actual shared-face reconstruction differences**,
//! attributes compaction / exact-zero delta dropping / density coarsening
//! **separately**, and reports savings both with and without a protection
//! stand-in — never touching a single emitted `.prl` byte.
//!
//! See `context/lib/experimental_spikes.md`: a spike cuts scope and hardening,
//! not rigor. This pass runs entirely CPU-side in the compiler, per 4×4×4 brick
//! incrementally, and never materializes the whole-map dense composed atlas.
//!
//! ## Three candidate stored levels per 4×4×4 brick
//! - **L0** — all 64 base probes (dense; ground truth).
//! - **L1** — the 8 corner probes (in-brick local ∈ {0,3}³), trilinear
//!   reconstruction with per-axis weights `local/3`.
//! - **L2** — a single brick-mean tile over the brick's VALID probes only.
//!
//! ## Metric definitions (judgment calls flagged for confirmation)
//! - **Per-texel error scalar**: max absolute per-channel RGB difference
//!   (irradiance units) between a reconstruction and its ground-truth tile,
//!   over the 4×4 octahedral interior texels. Invalid probes are excluded from
//!   both the reconstruction basis and the scored set.
//! - **Composed receiver tile**: base indirect (id 34) + base direct (id 35) +
//!   Σ id 27 + Σ id 41 + Σ id 45 delta tiles, summed per octahedral texel, at
//!   the STORED (peak / unit-radiance) delta magnitudes. Runtime color/intensity
//!   scaling is deliberately NOT applied — this analyses the stored payload.
//! - **Hemisphere/cosine weighting (metric 2b)**: each octahedral interior
//!   texel is weighted by the differential solid angle it subtends under the
//!   octahedral parameterization (numerical jacobian of a standard octahedral
//!   decode). Equal-area maps would collapse this to the unweighted form; the
//!   engine map is not equal-area, so the two differ.
//! - **Coarsenability gate (sweep)**: a brick takes the COARSEST level whose
//!   composed **unweighted max** per-texel error is ≤ the threshold. Max (not
//!   mean/p95) is the conservative "no texel worse than t" guarantee.
//! - **Seam error**: for each face-adjacent brick pair, at the shared boundary
//!   the reconstruction residual `recon − truth` is computed on each brick's own
//!   boundary probe layer under that brick's candidate level; the seam is the
//!   magnitude of the difference of the two bricks' residuals. This isolates the
//!   coarsening-induced discontinuity from the genuine lighting gradient across
//!   the boundary (a raw reconstructed-value diff is also reported).

use std::path::Path;

use glam::Vec3;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::octahedral::irradiance_array_tile_location;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use serde::Serialize;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::sh_bake::f16_bits_to_f32;

const AF: usize = AFFINITY_FACTOR as usize; // 4
const PROBES_PER_CELL: usize = AF * AF * AF; // 64

/// Exact-zero drop oracle epsilon: a delta entry counts as an exact-zero drop
/// candidate when every stored half is bit-zero. A wider near-zero fraction is
/// reported separately at [`NEAR_ZERO_EPS`].
const NEAR_ZERO_EPS: f32 = 1.0e-4;

/// Default composed-error thresholds swept in the histogram/savings table.
/// Irradiance units; seeded to bracket the plausible coarsenability band.
pub const DEFAULT_THRESHOLDS: [f32; 8] =
    [0.001, 0.005, 0.01, 0.02, 0.05, 0.1, 0.25, 0.5];

// ---------------------------------------------------------------------------
// Public inputs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ProtectAabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A field-level view over one of the three delta sections (ids 27/41/45),
/// which share the affinity-CSR layout.
struct DeltaView<'a> {
    affinity_dims: [u32; 3],
    tile_dimension: usize,
    tile_border: usize,
    offsets: &'a [u32],
    subblocks: &'a [u16],
}

impl<'a> DeltaView<'a> {
    fn from_indirect(s: &'a DeltaShVolumesSection) -> Self {
        Self {
            affinity_dims: s.affinity_dims,
            tile_dimension: s.tile_dimension as usize,
            tile_border: s.tile_border as usize,
            offsets: &s.affinity_offsets,
            subblocks: &s.delta_subblocks,
        }
    }
    fn from_direct(s: &'a DirectShDeltaVolumesSection) -> Self {
        Self {
            affinity_dims: s.affinity_dims,
            tile_dimension: s.tile_dimension as usize,
            tile_border: s.tile_border as usize,
            offsets: &s.affinity_offsets,
            subblocks: &s.delta_subblocks,
        }
    }
    fn from_anim_direct(s: &'a AnimatedDirectShDeltaVolumesSection) -> Self {
        Self {
            affinity_dims: s.affinity_dims,
            tile_dimension: s.tile_dimension as usize,
            tile_border: s.tile_border as usize,
            offsets: &s.affinity_offsets,
            subblocks: &s.delta_subblocks,
        }
    }

    fn probe_f16_stride(&self) -> usize {
        self.tile_dimension * self.tile_dimension * 4
    }
    fn subblock_stride(&self) -> usize {
        PROBES_PER_CELL * self.probe_f16_stride()
    }
    fn entry_count(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0) as usize
    }
}

pub struct AnalyzeInputs<'a> {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dims: [u32; 3],
    /// Per-probe validity (len == product(grid_dims)), x-fastest.
    pub validity: &'a [u8],
    /// Base indirect (id 34), captured pre-BC6H so `compact_atlas` is RGBA16F.
    pub base_indirect: &'a OctahedralShVolumeSection,
    /// Base direct (id 35), captured pre-BC6H so `atlas` is RGBA16F.
    pub base_direct: Option<&'a DirectShVolumeSection>,
    pub delta_indirect: Option<&'a DeltaShVolumesSection>,
    pub delta_direct: Option<&'a DirectShDeltaVolumesSection>,
    pub delta_anim_direct: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub protect_aabbs: &'a [ProtectAabb],
    pub thresholds: &'a [f32],
}

// ---------------------------------------------------------------------------
// Tile primitives
// ---------------------------------------------------------------------------

/// An octahedral interior tile: `interior*interior` RGB texels.
type Tile = Vec<Vec3>;

fn zero_tile(texels: usize) -> Tile {
    vec![Vec3::ZERO; texels]
}

// ---------------------------------------------------------------------------
// Solid-angle weights (metric 2b)
// ---------------------------------------------------------------------------

/// Standard octahedral decode (Cigolle et al. 2014) from square [-1,1]^2 to a
/// unit direction. Used only to derive a solid-angle weight per interior texel;
/// not required to match the engine's exact axis convention, since the weight
/// depends on the area-distortion profile of the octahedral map, not the axes.
fn oct_decode(p: f32, q: f32) -> Vec3 {
    let mut n = Vec3::new(p, q, 1.0 - p.abs() - q.abs());
    if n.z < 0.0 {
        let x = (1.0 - q.abs()) * p.signum();
        let y = (1.0 - p.abs()) * q.signum();
        n.x = x;
        n.y = y;
    }
    n.normalize()
}

/// Per-interior-texel differential solid-angle weights (unnormalized), computed
/// from the jacobian |∂dir/∂p × ∂dir/∂q| at each texel center.
fn solid_angle_weights(interior: usize) -> Vec<f32> {
    let mut w = Vec::with_capacity(interior * interior);
    let h = 1.0e-3f32;
    for iy in 0..interior {
        for ix in 0..interior {
            let u = (ix as f32 + 0.5) / interior as f32;
            let v = (iy as f32 + 0.5) / interior as f32;
            let p = u * 2.0 - 1.0;
            let q = v * 2.0 - 1.0;
            let dp = (oct_decode(p + h, q) - oct_decode(p - h, q)) / (2.0 * h);
            let dq = (oct_decode(p, q + h) - oct_decode(p, q - h)) / (2.0 * h);
            w.push(dp.cross(dq).length().max(1.0e-9));
        }
    }
    w
}

// ---------------------------------------------------------------------------
// Reconstruction math (unit-tested)
// ---------------------------------------------------------------------------

/// The 8 corner local indices of a 4×4×4 brick: local ∈ {0,3}³, x-fastest.
fn corner_locals() -> [usize; 8] {
    let mut out = [0usize; 8];
    let mut k = 0;
    for &cz in &[0usize, AF - 1] {
        for &cy in &[0usize, AF - 1] {
            for &cx in &[0usize, AF - 1] {
                out[k] = cx + cy * AF + cz * AF * AF;
                k += 1;
            }
        }
    }
    out
}

fn local_xyz(local: usize) -> (usize, usize, usize) {
    (local % AF, (local / AF) % AF, local / (AF * AF))
}

/// Trilinear weight of corner `(cx,cy,cz)∈{0,3}³` for a target at local
/// `(tx,ty,tz)`, per-axis weight = position along the 0..3 span.
fn trilinear_weight(target: (usize, usize, usize), corner: (usize, usize, usize)) -> f32 {
    let axis = |t: usize, c: usize| -> f32 {
        let f = t as f32 / (AF - 1) as f32; // 0..1 along the brick span
        if c == AF - 1 { f } else { 1.0 - f }
    };
    axis(target.0, corner.0) * axis(target.1, corner.1) * axis(target.2, corner.2)
}

/// L1 reconstruction of the tile at `target_local` from the brick's valid corner
/// tiles. Corners that are absent/invalid are dropped and the surviving weights
/// renormalized. Returns `None` when no valid corner exists.
fn reconstruct_l1_tile(
    tiles: &[Option<Tile>; PROBES_PER_CELL],
    target_local: usize,
    texels: usize,
) -> Option<Tile> {
    let target = local_xyz(target_local);
    let mut acc = zero_tile(texels);
    let mut wsum = 0.0f32;
    for corner_local in corner_locals() {
        if let Some(tile) = &tiles[corner_local] {
            let w = trilinear_weight(target, local_xyz(corner_local));
            if w <= 0.0 {
                continue;
            }
            for (a, t) in acc.iter_mut().zip(tile.iter()) {
                *a += *t * w;
            }
            wsum += w;
        }
    }
    if wsum <= 0.0 {
        return None;
    }
    for a in acc.iter_mut() {
        *a /= wsum;
    }
    Some(acc)
}

/// L2 brick-mean tile over valid probes. `None` when the brick has no valid
/// probe.
fn reconstruct_l2_tile(tiles: &[Option<Tile>; PROBES_PER_CELL], texels: usize) -> Option<Tile> {
    let mut acc = zero_tile(texels);
    let mut n = 0u32;
    for tile in tiles.iter().flatten() {
        for (a, t) in acc.iter_mut().zip(tile.iter()) {
            *a += *t;
        }
        n += 1;
    }
    if n == 0 {
        return None;
    }
    for a in acc.iter_mut() {
        *a /= n as f32;
    }
    Some(acc)
}

fn texel_error(recon: &Vec3, truth: &Vec3) -> f32 {
    (recon.x - truth.x)
        .abs()
        .max((recon.y - truth.y).abs())
        .max((recon.z - truth.z).abs())
}

// ---------------------------------------------------------------------------
// Streaming stats
// ---------------------------------------------------------------------------

/// Weighted/unweighted error accumulation over a set of texel samples.
#[derive(Default, Clone)]
struct ErrAccum {
    values: Vec<f32>,
    weights: Vec<f32>,
}

impl ErrAccum {
    fn push(&mut self, err: f32, weight: f32) {
        self.values.push(err);
        self.weights.push(weight);
    }
    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    fn max(&self) -> f32 {
        self.values.iter().copied().fold(0.0, f32::max)
    }
    fn mean(&self) -> f32 {
        if self.values.is_empty() {
            return 0.0;
        }
        self.values.iter().sum::<f32>() / self.values.len() as f32
    }
    fn weighted_mean(&self) -> f32 {
        let wsum: f32 = self.weights.iter().sum();
        if wsum <= 0.0 {
            return 0.0;
        }
        self.values
            .iter()
            .zip(&self.weights)
            .map(|(e, w)| e * w)
            .sum::<f32>()
            / wsum
    }
    fn p95(&self) -> f32 {
        percentile(&mut self.values.clone(), 0.95)
    }
    fn weighted_p95(&self) -> f32 {
        weighted_percentile(&self.values, &self.weights, 0.95)
    }
}

fn percentile(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((v.len() as f32 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn weighted_percentile(values: &[f32], weights: &[f32], p: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut pairs: Vec<(f32, f32)> =
        values.iter().copied().zip(weights.iter().copied()).collect();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let total: f32 = pairs.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        return pairs.last().map(|(v, _)| *v).unwrap_or(0.0);
    }
    let target = total * p;
    let mut cum = 0.0;
    for (v, w) in &pairs {
        cum += *w;
        if cum >= target {
            return *v;
        }
    }
    pairs.last().map(|(v, _)| *v).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Report structures (JSON-serialized)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
pub struct LevelErrStats {
    pub max: f32,
    pub mean: f32,
    pub p95: f32,
    pub weighted_mean: f32,
    pub weighted_p95: f32,
    /// Texel samples that contributed (valid probes × interior texels).
    pub texel_samples: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct BrickRecord {
    pub cell: [u32; 3],
    pub linear_cell: u32,
    pub valid_probes: u32,
    pub in_bounds_probes: u32,
    pub protected: bool,
    /// Base-irradiance reconstruction error (metric 1).
    pub base_l1: LevelErrStats,
    pub base_l2: LevelErrStats,
    /// Composed-receiver reconstruction error (metric 2, PRIMARY).
    pub composed_l1: LevelErrStats,
    pub composed_l2: LevelErrStats,
    pub world_min: [f32; 3],
    pub world_max: [f32; 3],
}

#[derive(Serialize, Clone, Default)]
pub struct AggregateErr {
    /// Max over per-brick maxes.
    pub max: f32,
    /// Texel-count-weighted mean over bricks.
    pub mean: f32,
    /// 95th percentile of per-brick mean errors.
    pub p95_of_brick_means: f32,
    pub weighted_mean: f32,
    pub bricks: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct SectionBytes {
    pub id: u32,
    /// Current uniform-density baseline (dense: every grid probe / every entry's
    /// full 64-probe subblock).
    pub uniform_bytes: u64,
    /// Line (a): after atlas compaction (dropping invalid probes).
    pub compacted_bytes: u64,
    /// Line (b): bytes recoverable by dropping exact-zero delta entries
    /// (delta sections only; 0 for id 34).
    pub exact_zero_dropped_bytes: u64,
    /// Line (c) bound: every non-empty brick coarsened to all-L1.
    pub coarsen_all_l1_bytes: u64,
    /// Line (c) bound: every non-empty brick coarsened to all-L2 (structural
    /// floor).
    pub coarsen_all_l2_bytes: u64,
    pub compacted_ratio: f32,
    pub coarsen_all_l1_ratio: f32,
    pub coarsen_all_l2_ratio: f32,
}

#[derive(Serialize, Clone, Default)]
pub struct SweepRow {
    pub threshold: f32,
    pub l0: u64,
    pub l1: u64,
    pub l2: u64,
    pub l0_protected: u64,
    pub l1_protected: u64,
    pub l2_protected: u64,
    /// Total projected bytes across ids 34/27/41/45 + composed atlas, no
    /// protection.
    pub projected_bytes: u64,
    /// Same with protection forcing intersecting bricks to L0.
    pub projected_bytes_protected: u64,
    pub ratio_to_uniform: f32,
    pub ratio_to_uniform_protected: f32,
}

#[derive(Serialize, Clone, Default)]
pub struct SeamStats {
    pub pairs: u64,
    pub cross_level_pairs: u64,
    /// Residual-difference seam (coarsening-induced discontinuity), the primary
    /// seam metric.
    pub residual_max: f32,
    pub residual_mean: f32,
    /// Raw reconstructed-value difference across the boundary (includes the true
    /// lighting gradient — contextual, not the isolated seam).
    pub raw_max: f32,
    pub raw_mean: f32,
    pub cross_level_residual_max: f32,
    pub cross_level_residual_mean: f32,
}

#[derive(Serialize, Clone, Default)]
pub struct AnalysisReport {
    pub grid_dims: [u32; 3],
    pub cell_size: [f32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub interior: u32,
    pub total_probes: u64,
    pub valid_probes: u64,
    pub brick_count: u64,
    pub nonempty_bricks: u64,
    pub protected_bricks: u64,
    pub has_base_direct: bool,
    pub has_delta_indirect: bool,
    pub has_delta_direct: bool,
    pub has_delta_anim_direct: bool,

    pub base_l1_aggregate: AggregateErr,
    pub base_l2_aggregate: AggregateErr,
    pub composed_l1_aggregate: AggregateErr,
    pub composed_l2_aggregate: AggregateErr,
    pub composed_l1_weighted_aggregate: AggregateErr,
    pub composed_l2_weighted_aggregate: AggregateErr,

    pub section_bytes: Vec<SectionBytes>,
    pub composed_atlas: SectionBytes,

    /// Exact-zero delta entry fraction, aggregated across the three delta
    /// sections (drop oracle).
    pub exact_zero_entry_fraction: f32,
    pub near_zero_entry_fraction: f32,
    pub near_zero_eps: f32,
    pub total_delta_entries: u64,

    pub seam: SeamStats,
    pub sweep: Vec<SweepRow>,

    pub protect_aabbs: Vec<[f32; 6]>,

    pub bricks: Vec<BrickRecord>,
}

// ---------------------------------------------------------------------------
// Byte model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    L0,
    L1,
    L2,
}

/// Stored tile count for a brick at a given level, intersected with validity.
fn stored_tiles(level: Level, valid_mask: &[bool; PROBES_PER_CELL]) -> usize {
    match level {
        Level::L0 => valid_mask.iter().filter(|&&v| v).count(),
        Level::L1 => corner_locals()
            .iter()
            .filter(|&&l| valid_mask[l])
            .count(),
        Level::L2 => {
            if valid_mask.iter().any(|&v| v) {
                1
            } else {
                0
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tile decoding
// ---------------------------------------------------------------------------

/// Decode one probe's interior RGB tile from the base indirect COMPACT atlas
/// (RGBA16F). Returns `None` for invalid probes (absent from the compact atlas).
fn decode_base_indirect_tile(
    section: &OctahedralShVolumeSection,
    valid_rank: i64,
    interior: usize,
    border: usize,
) -> Option<Tile> {
    if valid_rank < 0 {
        return None;
    }
    let tile_dim = section.tile_dimension as usize;
    let width = section.compact_atlas_dimensions[0] as usize;
    let height = section.compact_atlas_dimensions[1] as usize;
    let [layer, tx, ty] = irradiance_array_tile_location(
        valid_rank as usize,
        section.compact_atlas_tiles_per_layer,
        section.compact_atlas_tiles_per_row,
    );
    let layer_off = layer as usize * width * height;
    let ox = tx as usize * tile_dim;
    let oy = ty as usize * tile_dim;
    let bytes = &section.compact_atlas;
    let mut tile = zero_tile(interior * interior);
    for iy in 0..interior {
        for ix in 0..interior {
            let ax = ox + border + ix;
            let ay = oy + border + iy;
            let texel = layer_off + ay * width + ax;
            let byte = texel * 8;
            if byte + 8 > bytes.len() {
                return None;
            }
            let r = f16_bits_to_f32(u16::from_le_bytes([bytes[byte], bytes[byte + 1]]));
            let g = f16_bits_to_f32(u16::from_le_bytes([bytes[byte + 2], bytes[byte + 3]]));
            let b = f16_bits_to_f32(u16::from_le_bytes([bytes[byte + 4], bytes[byte + 5]]));
            tile[iy * interior + ix] = Vec3::new(r, g, b);
        }
    }
    Some(tile)
}

/// Decode one probe's interior RGB tile from the base DIRECT dense atlas
/// (RGBA16F), keyed by dense probe index.
fn decode_base_direct_tile(
    section: &DirectShVolumeSection,
    probe_index: usize,
    interior: usize,
    border: usize,
) -> Option<Tile> {
    let tile_dim = section.tile_dimension as usize;
    let width = section.atlas_dimensions[0] as usize;
    let height = section.atlas_dimensions[1] as usize;
    let [layer, tx, ty] = irradiance_array_tile_location(
        probe_index,
        section.tiles_per_layer,
        section.atlas_tiles_per_row,
    );
    let layer_off = layer as usize * width * height;
    let ox = tx as usize * tile_dim;
    let oy = ty as usize * tile_dim;
    let bytes = &section.atlas;
    let mut tile = zero_tile(interior * interior);
    for iy in 0..interior {
        for ix in 0..interior {
            let ax = ox + border + ix;
            let ay = oy + border + iy;
            let texel = layer_off + ay * width + ax;
            let byte = texel * 8;
            if byte + 8 > bytes.len() {
                return None;
            }
            let r = f16_bits_to_f32(u16::from_le_bytes([bytes[byte], bytes[byte + 1]]));
            let g = f16_bits_to_f32(u16::from_le_bytes([bytes[byte + 2], bytes[byte + 3]]));
            let b = f16_bits_to_f32(u16::from_le_bytes([bytes[byte + 4], bytes[byte + 5]]));
            tile[iy * interior + ix] = Vec3::new(r, g, b);
        }
    }
    Some(tile)
}

/// Accumulate one delta section's contribution for a whole brick into a
/// per-local-probe tile array (adds onto `acc`). `cell` is the affinity-cell
/// linear index in the SECTION's own affinity grid.
fn accumulate_delta_for_cell(
    view: &DeltaView<'_>,
    cell: usize,
    interior: usize,
    border: usize,
    acc: &mut [Tile; PROBES_PER_CELL],
) {
    if cell + 1 >= view.offsets.len() {
        return;
    }
    let tile_dim = view.tile_dimension;
    let probe_stride = view.probe_f16_stride();
    let subblock_stride = view.subblock_stride();
    let start = view.offsets[cell] as usize;
    let end = view.offsets[cell + 1] as usize;
    for entry in start..end {
        let base = entry * subblock_stride;
        for local in 0..PROBES_PER_CELL {
            let probe_base = base + local * probe_stride;
            for iy in 0..interior {
                for ix in 0..interior {
                    let full = ((border + iy) * tile_dim + (border + ix)) * 4;
                    let idx = probe_base + full;
                    if idx + 3 >= view.subblocks.len() {
                        continue;
                    }
                    let r = f16_bits_to_f32(view.subblocks[idx]);
                    let g = f16_bits_to_f32(view.subblocks[idx + 1]);
                    let b = f16_bits_to_f32(view.subblocks[idx + 2]);
                    acc[local][iy * interior + ix] += Vec3::new(r, g, b);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-brick working set
// ---------------------------------------------------------------------------

struct BrickTiles {
    /// Composed truth tile per local probe (Some iff in-bounds AND valid).
    composed: [Option<Tile>; PROBES_PER_CELL],
    /// Base-indirect truth tile per local probe.
    base: [Option<Tile>; PROBES_PER_CELL],
    valid_mask: [bool; PROBES_PER_CELL],
    in_bounds: [bool; PROBES_PER_CELL],
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run_analysis(inputs: &AnalyzeInputs<'_>) -> AnalysisReport {
    let dims = inputs.grid_dims;
    let base = inputs.base_indirect;
    let tile_dim = base.tile_dimension as usize;
    let border = base.tile_border as usize;
    let interior = tile_dim.saturating_sub(2 * border);
    let texels = interior * interior;

    let mut report = AnalysisReport {
        grid_dims: dims,
        cell_size: inputs.cell_size,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        interior: interior as u32,
        has_base_direct: inputs.base_direct.is_some(),
        has_delta_indirect: inputs.delta_indirect.is_some(),
        has_delta_direct: inputs.delta_direct.is_some(),
        has_delta_anim_direct: inputs.delta_anim_direct.is_some(),
        protect_aabbs: inputs
            .protect_aabbs
            .iter()
            .map(|a| [a.min[0], a.min[1], a.min[2], a.max[0], a.max[1], a.max[2]])
            .collect(),
        ..Default::default()
    };

    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
    let total_probes = nx * ny * nz;
    if total_probes == 0 || interior == 0 {
        log::warn!("[sh-analyze] empty grid or degenerate tile geometry; nothing to analyze");
        return report;
    }
    report.total_probes = total_probes as u64;

    // Per-probe valid-rank map (compact-atlas slot) — x-fastest.
    let mut valid_rank = vec![-1i64; total_probes];
    let mut rank = 0i64;
    for (i, r) in valid_rank.iter_mut().enumerate() {
        if inputs.validity.get(i).copied().unwrap_or(0) != 0 {
            *r = rank;
            rank += 1;
        }
    }
    report.valid_probes = rank as u64;

    let weights = solid_angle_weights(interior);

    // Affinity grid (bricks). ceil(dims/4).
    let ax = (nx + AF - 1) / AF;
    let ay = (ny + AF - 1) / AF;
    let az = (nz + AF - 1) / AF;
    let brick_count = ax * ay * az;
    report.brick_count = brick_count as u64;

    let expected_affinity = [ax as u32, ay as u32, az as u32];
    let delta_indirect = check_delta(inputs.delta_indirect.map(DeltaView::from_indirect), expected_affinity, "id27");
    let delta_direct = check_delta(inputs.delta_direct.map(DeltaView::from_direct), expected_affinity, "id41");
    let delta_anim = check_delta(inputs.delta_anim_direct.map(DeltaView::from_anim_direct), expected_affinity, "id45");

    // Aggregate accumulators.
    let mut base_l1_agg = AggAcc::default();
    let mut base_l2_agg = AggAcc::default();
    let mut comp_l1_agg = AggAcc::default();
    let mut comp_l2_agg = AggAcc::default();
    let mut comp_l1_w_agg = AggAcc::default();
    let mut comp_l2_w_agg = AggAcc::default();

    // Sweep counters.
    let thresholds = inputs.thresholds;
    let mut sweep_counts: Vec<[u64; 6]> = vec![[0; 6]; thresholds.len()]; // l0,l1,l2, l0p,l1p,l2p

    // Byte accumulators, per (level assignment) for the sweep, per section.
    // We accumulate stored-tile counts; bytes = tiles * probe_tile_bytes.
    let probe_tile_bytes = (tile_dim * tile_dim * 4 * 2) as u64; // RGBA16F full tile

    // Section byte lines.
    let mut base_uniform_tiles = 0u64;
    let mut base_compacted_tiles = 0u64;
    let mut base_l1_tiles = 0u64;
    let mut base_l2_tiles = 0u64;

    // For the sweep projected bytes we need, per threshold, the chosen level per
    // brick and the resulting stored tiles for base + composed + each delta
    // section. We compute delta stored tiles per brick per level too.
    let mut sweep_base_tiles: Vec<[u64; 2]> = vec![[0; 2]; thresholds.len()]; // [no-protect, protect]
    let mut sweep_delta_tiles: Vec<[u64; 2]> = vec![[0; 2]; thresholds.len()];

    // Delta uniform/compacted/coarsen accumulators (aggregate across 3 sections).
    let mut delta_uniform_tiles = 0u64; // entries * 64
    let mut delta_compacted_tiles = 0u64; // entries * valid-in-cell
    let mut delta_l1_tiles = 0u64;
    let mut delta_l2_tiles = 0u64;
    let mut delta_exact_zero_entries = 0u64;
    let mut delta_near_zero_entries = 0u64;
    let mut delta_total_entries = 0u64;

    // Per-section entry totals for exact-zero byte line.
    let mut per_section_entries: [u64; 3] = [0; 3];
    let mut per_section_exact_zero: [u64; 3] = [0; 3];

    // Seam cache: per brick, store per-local composed truth + reconstructed
    // (L1,L2) tiles ONLY for boundary layers, plus the brick's per-threshold
    // chosen level is recomputed from stored composed max error. To keep it
    // simple and rigorous we store, per brick: the composed truth tiles for all
    // in-bounds locals is too big map-wide. We instead store a compact
    // SeamBrick capturing, for each of the 3 max-face layers and 3 min-face
    // layers, the 16 boundary probes' (truth, reconL1, reconL2, valid) — bounded
    // to 6*16 tiles per brick, kept only transiently and consumed by the seam
    // pass which streams pairs. Given brick counts can be large, we hold these
    // in a Vec sized to brick_count; each SeamBrick is a few KB. For the 10 m
    // deliverable run this is small. For very large grids this is the dominant
    // cost but still O(surface tiles), not the dense composed atlas.
    let mut seam_bricks: Vec<SeamBrick> = vec![SeamBrick::default(); brick_count];

    // Per-brick composed max error at L1/L2 for the sweep gate.
    let mut brick_comp_l1_max: Vec<f32> = vec![f32::INFINITY; brick_count];
    let mut brick_comp_l2_max: Vec<f32> = vec![f32::INFINITY; brick_count];
    let mut brick_valid_masks: Vec<[bool; PROBES_PER_CELL]> = vec![[false; PROBES_PER_CELL]; brick_count];
    let mut brick_protected: Vec<bool> = vec![false; brick_count];
    let mut brick_nonempty: Vec<bool> = vec![false; brick_count];

    // Per-brick delta entry count and per-level delta stored tiles (aggregate
    // over the 3 sections) for the sweep byte projection.
    let mut brick_delta_l0_tiles: Vec<u64> = vec![0; brick_count];
    let mut brick_delta_l1_tiles: Vec<u64> = vec![0; brick_count];
    let mut brick_delta_l2_tiles: Vec<u64> = vec![0; brick_count];

    let mut nonempty_bricks = 0u64;
    let mut protected_bricks = 0u64;

    for cz in 0..az {
        for cy in 0..ay {
            for cx in 0..ax {
                let cell_lin = cx + cy * ax + cz * ax * ay;
                let bt = build_brick_tiles(
                    inputs, base, tile_dim, interior, border, &valid_rank, dims, cell_lin, cx, cy,
                    cz, ax, ay, &delta_indirect, &delta_direct, &delta_anim,
                );
                let valid_probes = bt.valid_mask.iter().filter(|&&v| v).count() as u32;
                let in_bounds = bt.in_bounds.iter().filter(|&&v| v).count() as u32;
                brick_valid_masks[cell_lin] = bt.valid_mask;

                // World AABB of the brick from its in-bounds probe positions.
                let (wmin, wmax) = brick_world_aabb(inputs, dims, cx, cy, cz);
                let protected = intersects_any(inputs.protect_aabbs, wmin, wmax);
                brick_protected[cell_lin] = protected;
                if protected {
                    protected_bricks += 1;
                }

                if valid_probes == 0 {
                    // Empty brick: contributes nothing to bytes/errors.
                    continue;
                }
                brick_nonempty[cell_lin] = true;
                nonempty_bricks += 1;

                // --- Base + composed reconstruction errors ---
                let base_l1 = level_errors(&bt.base, LevelKind::L1, texels, interior, &weights);
                let base_l2 = level_errors(&bt.base, LevelKind::L2, texels, interior, &weights);
                let comp_l1 = level_errors(&bt.composed, LevelKind::L1, texels, interior, &weights);
                let comp_l2 = level_errors(&bt.composed, LevelKind::L2, texels, interior, &weights);

                base_l1_agg.push(&base_l1);
                base_l2_agg.push(&base_l2);
                comp_l1_agg.push(&comp_l1);
                comp_l2_agg.push(&comp_l2);
                comp_l1_w_agg.push_weighted(&comp_l1);
                comp_l2_w_agg.push_weighted(&comp_l2);

                brick_comp_l1_max[cell_lin] = comp_l1.max;
                brick_comp_l2_max[cell_lin] = comp_l2.max;

                // --- Byte lines (base) ---
                base_uniform_tiles += PROBES_PER_CELL as u64; // dense brick
                base_compacted_tiles += stored_tiles(Level::L0, &bt.valid_mask) as u64;
                base_l1_tiles += stored_tiles(Level::L1, &bt.valid_mask) as u64;
                base_l2_tiles += stored_tiles(Level::L2, &bt.valid_mask) as u64;

                // --- Seam cache (boundary layers) ---
                seam_bricks[cell_lin] =
                    build_seam_brick(&bt, texels, interior, &weights);

                // --- Per-brick record ---
                report.bricks.push(BrickRecord {
                    cell: [cx as u32, cy as u32, cz as u32],
                    linear_cell: cell_lin as u32,
                    valid_probes,
                    in_bounds_probes: in_bounds,
                    protected,
                    base_l1: base_l1.to_stats(),
                    base_l2: base_l2.to_stats(),
                    composed_l1: comp_l1.to_stats(),
                    composed_l2: comp_l2.to_stats(),
                    world_min: [wmin.x, wmin.y, wmin.z],
                    world_max: [wmax.x, wmax.y, wmax.z],
                });
            }
        }
    }

    // --- Delta byte lines + exact-zero oracle (aggregate over 3 sections) ---
    for (si, view) in [&delta_indirect, &delta_direct, &delta_anim]
        .into_iter()
        .enumerate()
    {
        let Some(view) = view else { continue };
        let entries = view.entry_count() as u64;
        delta_total_entries += entries;
        per_section_entries[si] = entries;
        delta_uniform_tiles += entries * PROBES_PER_CELL as u64;

        // Walk each cell's entries: exact-zero oracle + per-level compacted
        // stored tiles (using the cell's brick valid mask).
        for c in 0..view.offsets.len().saturating_sub(1) {
            // Map section cell -> our brick cell (same affinity grid).
            let mask = brick_valid_masks.get(c).copied().unwrap_or([false; PROBES_PER_CELL]);
            let l0 = stored_tiles(Level::L0, &mask) as u64;
            let l1 = stored_tiles(Level::L1, &mask) as u64;
            let l2 = stored_tiles(Level::L2, &mask) as u64;
            let start = view.offsets[c] as usize;
            let end = view.offsets[c + 1] as usize;
            for entry in start..end {
                let (exact_zero, near_zero) = entry_zeroness(view, entry);
                if exact_zero {
                    delta_exact_zero_entries += 1;
                    per_section_exact_zero[si] += 1;
                }
                if near_zero {
                    delta_near_zero_entries += 1;
                }
                // Compacted (L0) / coarsen bounds accumulate only for retained
                // (non-exact-zero) entries.
                if !exact_zero {
                    delta_compacted_tiles += l0;
                    delta_l1_tiles += l1;
                    delta_l2_tiles += l2;
                    brick_delta_l0_tiles[c] += l0;
                    brick_delta_l1_tiles[c] += l1;
                    brick_delta_l2_tiles[c] += l2;
                }
            }
        }
    }

    report.nonempty_bricks = nonempty_bricks;
    report.protected_bricks = protected_bricks;
    report.total_delta_entries = delta_total_entries;
    report.exact_zero_entry_fraction = frac(delta_exact_zero_entries, delta_total_entries);
    report.near_zero_entry_fraction = frac(delta_near_zero_entries, delta_total_entries);
    report.near_zero_eps = NEAR_ZERO_EPS;

    report.base_l1_aggregate = base_l1_agg.finish();
    report.base_l2_aggregate = base_l2_agg.finish();
    report.composed_l1_aggregate = comp_l1_agg.finish();
    report.composed_l2_aggregate = comp_l2_agg.finish();
    report.composed_l1_weighted_aggregate = comp_l1_w_agg.finish();
    report.composed_l2_weighted_aggregate = comp_l2_w_agg.finish();

    // --- Section byte tables ---
    let mk = |id: u32, uni: u64, comp: u64, ez: u64, l1: u64, l2: u64| SectionBytes {
        id,
        uniform_bytes: uni * probe_tile_bytes,
        compacted_bytes: comp * probe_tile_bytes,
        exact_zero_dropped_bytes: ez * probe_tile_bytes,
        coarsen_all_l1_bytes: l1 * probe_tile_bytes,
        coarsen_all_l2_bytes: l2 * probe_tile_bytes,
        compacted_ratio: ratio(comp, uni),
        coarsen_all_l1_ratio: ratio(l1, uni),
        coarsen_all_l2_ratio: ratio(l2, uni),
    };
    // id 34 base.
    report.section_bytes.push(mk(
        34,
        base_uniform_tiles,
        base_compacted_tiles,
        0,
        base_l1_tiles,
        base_l2_tiles,
    ));
    // Delta sections: report the aggregate delta lines split per present id.
    // Each present delta id shares the aggregate compacted/coarsen profile
    // proportional to its entry share; we instead report each id with its own
    // uniform (entries*64) and its own exact-zero, and the shared compacted/
    // coarsen model applied to that id's entries.
    let delta_ids = [(27u32, &delta_indirect), (41, &delta_direct), (45, &delta_anim)];
    for (idx, (id, view)) in delta_ids.iter().enumerate() {
        let Some(view) = view else { continue };
        let mut uni = 0u64;
        let mut comp = 0u64;
        let mut l1 = 0u64;
        let mut l2 = 0u64;
        let mut ez = 0u64;
        for c in 0..view.offsets.len().saturating_sub(1) {
            let mask = brick_valid_masks.get(c).copied().unwrap_or([false; PROBES_PER_CELL]);
            let sl0 = stored_tiles(Level::L0, &mask) as u64;
            let sl1 = stored_tiles(Level::L1, &mask) as u64;
            let sl2 = stored_tiles(Level::L2, &mask) as u64;
            let start = view.offsets[c] as usize;
            let end = view.offsets[c + 1] as usize;
            for entry in start..end {
                uni += PROBES_PER_CELL as u64;
                let (exact_zero, _) = entry_zeroness(view, entry);
                if exact_zero {
                    ez += PROBES_PER_CELL as u64;
                } else {
                    comp += sl0;
                    l1 += sl1;
                    l2 += sl2;
                }
            }
        }
        let _ = idx;
        report.section_bytes.push(mk(*id, uni, comp, ez, l1, l2));
    }
    let _ = (
        delta_uniform_tiles,
        delta_compacted_tiles,
        delta_l1_tiles,
        delta_l2_tiles,
        per_section_entries,
        per_section_exact_zero,
    );

    // Composed atlas projection (dense per stored probe; same geometry as base).
    report.composed_atlas = SectionBytes {
        id: 0, // synthetic: composed runtime atlas
        uniform_bytes: base_uniform_tiles * probe_tile_bytes,
        compacted_bytes: base_compacted_tiles * probe_tile_bytes,
        exact_zero_dropped_bytes: 0,
        coarsen_all_l1_bytes: base_l1_tiles * probe_tile_bytes,
        coarsen_all_l2_bytes: base_l2_tiles * probe_tile_bytes,
        compacted_ratio: ratio(base_compacted_tiles, base_uniform_tiles),
        coarsen_all_l1_ratio: ratio(base_l1_tiles, base_uniform_tiles),
        coarsen_all_l2_ratio: ratio(base_l2_tiles, base_uniform_tiles),
    };

    // --- Threshold sweep ---
    // Uniform baseline for the ratio = base uniform + delta uniform + composed
    // uniform (dense everything).
    let uniform_total_tiles = base_uniform_tiles + delta_uniform_tiles + base_uniform_tiles;
    for (ti, &t) in thresholds.iter().enumerate() {
        let mut counts = [0u64; 6];
        let mut proj_tiles = 0u64;
        let mut proj_tiles_prot = 0u64;
        for b in 0..brick_count {
            if !brick_nonempty[b] {
                continue;
            }
            let mask = brick_valid_masks[b];
            let lvl = choose_level(brick_comp_l1_max[b], brick_comp_l2_max[b], t);
            // Base + composed stored tiles at chosen level (both share geometry).
            let base_t = stored_tiles(lvl, &mask) as u64;
            let delta_t = match lvl {
                Level::L0 => brick_delta_l0_tiles[b],
                Level::L1 => brick_delta_l1_tiles[b],
                Level::L2 => brick_delta_l2_tiles[b],
            };
            // no-protect
            match lvl {
                Level::L0 => counts[0] += 1,
                Level::L1 => counts[1] += 1,
                Level::L2 => counts[2] += 1,
            }
            proj_tiles += base_t * 2 + delta_t; // base + composed + delta

            // protected: intersecting bricks forced L0.
            let plvl = if brick_protected[b] { Level::L0 } else { lvl };
            let pbase_t = stored_tiles(plvl, &mask) as u64;
            let pdelta_t = match plvl {
                Level::L0 => brick_delta_l0_tiles[b],
                Level::L1 => brick_delta_l1_tiles[b],
                Level::L2 => brick_delta_l2_tiles[b],
            };
            match plvl {
                Level::L0 => counts[3] += 1,
                Level::L1 => counts[4] += 1,
                Level::L2 => counts[5] += 1,
            }
            proj_tiles_prot += pbase_t * 2 + pdelta_t;
        }
        sweep_counts[ti] = counts;
        sweep_base_tiles[ti] = [proj_tiles, proj_tiles_prot];
        let _ = &sweep_delta_tiles;
        report.sweep.push(SweepRow {
            threshold: t,
            l0: counts[0],
            l1: counts[1],
            l2: counts[2],
            l0_protected: counts[3],
            l1_protected: counts[4],
            l2_protected: counts[5],
            projected_bytes: proj_tiles * probe_tile_bytes,
            projected_bytes_protected: proj_tiles_prot * probe_tile_bytes,
            ratio_to_uniform: ratio(proj_tiles, uniform_total_tiles),
            ratio_to_uniform_protected: ratio(proj_tiles_prot, uniform_total_tiles),
        });
    }

    // --- Seam pass (shared-face reconstruction differencing) ---
    report.seam = compute_seams(
        &seam_bricks,
        &brick_nonempty,
        &brick_comp_l1_max,
        &brick_comp_l2_max,
        ax,
        ay,
        az,
        interior,
        // Seam evaluated at a representative mid threshold so bricks pick
        // realistic (possibly differing) levels.
        representative_threshold(thresholds),
    );

    report
}

fn frac(n: u64, d: u64) -> f32 {
    if d == 0 { 0.0 } else { n as f32 / d as f32 }
}
fn ratio(n: u64, d: u64) -> f32 {
    if d == 0 { 0.0 } else { n as f32 / d as f32 }
}

fn representative_threshold(thresholds: &[f32]) -> f32 {
    if thresholds.is_empty() {
        0.02
    } else {
        thresholds[thresholds.len() / 2]
    }
}

fn choose_level(l1_max: f32, l2_max: f32, t: f32) -> Level {
    if l2_max <= t {
        Level::L2
    } else if l1_max <= t {
        Level::L1
    } else {
        Level::L0
    }
}

fn check_delta<'a>(
    view: Option<DeltaView<'a>>,
    expected: [u32; 3],
    label: &str,
) -> Option<DeltaView<'a>> {
    if let Some(v) = &view {
        if v.affinity_dims != expected {
            log::warn!(
                "[sh-analyze] {label} affinity_dims {:?} != expected {:?}; skipping this delta section",
                v.affinity_dims,
                expected
            );
            return None;
        }
    }
    view
}

// ---------------------------------------------------------------------------
// Brick assembly
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_brick_tiles(
    inputs: &AnalyzeInputs<'_>,
    base: &OctahedralShVolumeSection,
    tile_dim: usize,
    interior: usize,
    border: usize,
    valid_rank: &[i64],
    dims: [u32; 3],
    cell_lin: usize,
    cx: usize,
    cy: usize,
    cz: usize,
    ax: usize,
    ay: usize,
    delta_indirect: &Option<DeltaView<'_>>,
    delta_direct: &Option<DeltaView<'_>>,
    delta_anim: &Option<DeltaView<'_>>,
) -> BrickTiles {
    let _ = tile_dim;
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
    let texels = interior * interior;

    const NONE_TILE: Option<Tile> = None;
    let mut base_tiles: [Option<Tile>; PROBES_PER_CELL] = [NONE_TILE; PROBES_PER_CELL];
    let mut composed: [Option<Tile>; PROBES_PER_CELL] = [NONE_TILE; PROBES_PER_CELL];
    let mut valid_mask = [false; PROBES_PER_CELL];
    let mut in_bounds = [false; PROBES_PER_CELL];

    // Delta contributions accumulated per local probe (zero tiles for all 64).
    let mut delta_acc: [Tile; PROBES_PER_CELL] =
        std::array::from_fn(|_| zero_tile(texels));
    let cell_for_section = cell_lin; // same affinity grid
    if let Some(v) = delta_indirect {
        accumulate_delta_for_cell(v, cell_for_section, interior, border, &mut delta_acc);
    }
    if let Some(v) = delta_direct {
        accumulate_delta_for_cell(v, cell_for_section, interior, border, &mut delta_acc);
    }
    if let Some(v) = delta_anim {
        accumulate_delta_for_cell(v, cell_for_section, interior, border, &mut delta_acc);
    }
    let _ = (ax, ay);

    for local in 0..PROBES_PER_CELL {
        let (lx, ly, lz) = local_xyz(local);
        let px = cx * AF + lx;
        let py = cy * AF + ly;
        let pz = cz * AF + lz;
        if px >= nx || py >= ny || pz >= nz {
            continue;
        }
        in_bounds[local] = true;
        let probe_index = px + py * nx + pz * nx * ny;
        let valid = valid_rank[probe_index] >= 0;
        valid_mask[local] = valid;
        if !valid {
            continue;
        }
        let base_ind = decode_base_indirect_tile(base, valid_rank[probe_index], interior, border);
        let Some(base_ind_tile) = base_ind else { continue };

        // Composed = base indirect + base direct + Σ deltas.
        let mut comp = base_ind_tile.clone();
        if let Some(dir) = inputs.base_direct {
            if let Some(dt) = decode_base_direct_tile(dir, probe_index, interior, border) {
                for (a, b) in comp.iter_mut().zip(dt.iter()) {
                    *a += *b;
                }
            }
        }
        for (a, d) in comp.iter_mut().zip(delta_acc[local].iter()) {
            *a += *d;
        }

        base_tiles[local] = Some(base_ind_tile);
        composed[local] = Some(comp);
    }

    BrickTiles {
        composed,
        base: base_tiles,
        valid_mask,
        in_bounds,
    }
}

fn brick_world_aabb(
    inputs: &AnalyzeInputs<'_>,
    dims: [u32; 3],
    cx: usize,
    cy: usize,
    cz: usize,
) -> (Vec3, Vec3) {
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
    let o = Vec3::from(inputs.grid_origin);
    let cs = Vec3::from(inputs.cell_size);
    let x0 = cx * AF;
    let y0 = cy * AF;
    let z0 = cz * AF;
    let x1 = (x0 + AF - 1).min(nx - 1);
    let y1 = (y0 + AF - 1).min(ny - 1);
    let z1 = (z0 + AF - 1).min(nz - 1);
    let wmin = o + Vec3::new(x0 as f32 * cs.x, y0 as f32 * cs.y, z0 as f32 * cs.z);
    let wmax = o + Vec3::new(x1 as f32 * cs.x, y1 as f32 * cs.y, z1 as f32 * cs.z);
    (wmin, wmax)
}

fn intersects_any(aabbs: &[ProtectAabb], wmin: Vec3, wmax: Vec3) -> bool {
    aabbs.iter().any(|a| {
        wmin.x <= a.max[0]
            && wmax.x >= a.min[0]
            && wmin.y <= a.max[1]
            && wmax.y >= a.min[1]
            && wmin.z <= a.max[2]
            && wmax.z >= a.min[2]
    })
}

// ---------------------------------------------------------------------------
// Level error computation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum LevelKind {
    L1,
    L2,
}

struct LevelErr {
    max: f32,
    mean: f32,
    p95: f32,
    weighted_mean: f32,
    weighted_p95: f32,
    texel_samples: u64,
}

impl LevelErr {
    fn to_stats(&self) -> LevelErrStats {
        LevelErrStats {
            max: self.max,
            mean: self.mean,
            p95: self.p95,
            weighted_mean: self.weighted_mean,
            weighted_p95: self.weighted_p95,
            texel_samples: self.texel_samples,
        }
    }
}

fn level_errors(
    tiles: &[Option<Tile>; PROBES_PER_CELL],
    kind: LevelKind,
    texels: usize,
    interior: usize,
    weights: &[f32],
) -> LevelErr {
    let _ = interior;
    let mut acc = ErrAccum::default();
    for target_local in 0..PROBES_PER_CELL {
        let Some(truth) = &tiles[target_local] else {
            continue;
        };
        let recon = match kind {
            LevelKind::L1 => reconstruct_l1_tile(tiles, target_local, texels),
            LevelKind::L2 => reconstruct_l2_tile(tiles, texels),
        };
        let Some(recon) = recon else { continue };
        for texel in 0..texels {
            let e = texel_error(&recon[texel], &truth[texel]);
            acc.push(e, weights[texel]);
        }
    }
    if acc.is_empty() {
        return LevelErr {
            max: 0.0,
            mean: 0.0,
            p95: 0.0,
            weighted_mean: 0.0,
            weighted_p95: 0.0,
            texel_samples: 0,
        };
    }
    LevelErr {
        max: acc.max(),
        mean: acc.mean(),
        p95: acc.p95(),
        weighted_mean: acc.weighted_mean(),
        weighted_p95: acc.weighted_p95(),
        texel_samples: acc.values.len() as u64,
    }
}

// ---------------------------------------------------------------------------
// Aggregate accumulator
// ---------------------------------------------------------------------------

#[derive(Default)]
struct AggAcc {
    max: f32,
    weighted_sum: f64,
    sum: f64,
    weight_total: f64,
    sample_total: u64,
    brick_means: Vec<f32>,
    bricks: u64,
}

impl AggAcc {
    fn push(&mut self, e: &LevelErr) {
        if e.texel_samples == 0 {
            return;
        }
        self.max = self.max.max(e.max);
        self.sum += e.mean as f64 * e.texel_samples as f64;
        self.sample_total += e.texel_samples;
        self.brick_means.push(e.mean);
        self.bricks += 1;
    }
    fn push_weighted(&mut self, e: &LevelErr) {
        if e.texel_samples == 0 {
            return;
        }
        self.max = self.max.max(e.max);
        self.weighted_sum += e.weighted_mean as f64 * e.texel_samples as f64;
        self.weight_total += e.texel_samples as f64;
        self.sum += e.weighted_mean as f64 * e.texel_samples as f64;
        self.sample_total += e.texel_samples;
        self.brick_means.push(e.weighted_mean);
        self.bricks += 1;
    }
    fn finish(mut self) -> AggregateErr {
        let mean = if self.sample_total > 0 {
            (self.sum / self.sample_total as f64) as f32
        } else {
            0.0
        };
        let weighted_mean = if self.weight_total > 0.0 {
            (self.weighted_sum / self.weight_total) as f32
        } else {
            mean
        };
        AggregateErr {
            max: self.max,
            mean,
            p95_of_brick_means: percentile(&mut self.brick_means, 0.95),
            weighted_mean,
            bricks: self.bricks,
        }
    }
}

// ---------------------------------------------------------------------------
// Exact-zero oracle
// ---------------------------------------------------------------------------

/// Returns (exact_zero, near_zero) for one delta CSR entry's 64-probe subblock.
fn entry_zeroness(view: &DeltaView<'_>, entry: usize) -> (bool, bool) {
    let stride = view.subblock_stride();
    let base = entry * stride;
    let end = (base + stride).min(view.subblocks.len());
    let mut all_zero = true;
    let mut max_abs = 0.0f32;
    for &h in &view.subblocks[base..end] {
        if h != 0 {
            all_zero = false;
        }
        let v = f16_bits_to_f32(h).abs();
        if v > max_abs {
            max_abs = v;
        }
    }
    (all_zero, max_abs < NEAR_ZERO_EPS)
}

// ---------------------------------------------------------------------------
// Seam pass
// ---------------------------------------------------------------------------

/// Boundary-layer residual/recon tiles for one brick, for the 3 max faces
/// (+x,+y,+z) and 3 min faces (−x,−y,−z). Each face carries the 16 boundary
/// probes' L1 and L2 residual (recon − truth) and truth, plus validity.
#[derive(Clone, Default)]
struct SeamBrick {
    /// For each face 0..6 (order: +x,+y,+z,−x,−y,−z), the boundary layer's
    /// per-position L1 residual, L2 residual, and truth. Indexed by the 16
    /// (a,b) face positions. Empty vec if brick empty.
    faces: [Vec<FacePos>; 6],
    nonempty: bool,
}

#[derive(Clone, Default)]
struct FacePos {
    valid: bool,
    l1_residual: Vec<Vec3>,
    l2_residual: Vec<Vec3>,
    l1_recon: Vec<Vec3>,
    l2_recon: Vec<Vec3>,
}

fn build_seam_brick(
    bt: &BrickTiles,
    texels: usize,
    _interior: usize,
    _weights: &[f32],
) -> SeamBrick {
    // Precompute L1/L2 recon per local (only for valid locals).
    let mut l1_recon: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
    let mut l2_recon: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
    let l2_tile = reconstruct_l2_tile(&bt.composed, texels);
    for local in 0..PROBES_PER_CELL {
        if bt.composed[local].is_none() {
            continue;
        }
        l1_recon[local] = reconstruct_l1_tile(&bt.composed, local, texels);
        l2_recon[local] = l2_tile.clone();
    }

    let mut sb = SeamBrick {
        nonempty: true,
        ..Default::default()
    };
    // Face definitions: (axis, is_max)
    let faces = [
        (0usize, true),
        (1, true),
        (2, true),
        (0, false),
        (1, false),
        (2, false),
    ];
    for (fi, (axis, is_max)) in faces.into_iter().enumerate() {
        let layer_coord = if is_max { AF - 1 } else { 0 };
        let mut positions = Vec::with_capacity(AF * AF);
        for b in 0..AF {
            for a in 0..AF {
                let (lx, ly, lz) = match axis {
                    0 => (layer_coord, a, b),
                    1 => (a, layer_coord, b),
                    _ => (a, b, layer_coord),
                };
                let local = lx + ly * AF + lz * AF * AF;
                let truth = bt.composed[local].clone();
                let (valid, l1r, l2r, l1c, l2c) = if let Some(truth) = truth {
                    let l1 = l1_recon[local].clone().unwrap_or_else(|| zero_tile(texels));
                    let l2 = l2_recon[local].clone().unwrap_or_else(|| zero_tile(texels));
                    let l1res: Vec<Vec3> =
                        l1.iter().zip(&truth).map(|(r, t)| *r - *t).collect();
                    let l2res: Vec<Vec3> =
                        l2.iter().zip(&truth).map(|(r, t)| *r - *t).collect();
                    (true, l1res, l2res, l1, l2)
                } else {
                    (false, Vec::new(), Vec::new(), Vec::new(), Vec::new())
                };
                positions.push(FacePos {
                    valid,
                    l1_residual: l1r,
                    l2_residual: l2r,
                    l1_recon: l1c,
                    l2_recon: l2c,
                });
            }
        }
        sb.faces[fi] = positions;
    }
    sb
}

#[allow(clippy::too_many_arguments)]
fn compute_seams(
    bricks: &[SeamBrick],
    nonempty: &[bool],
    l1_max: &[f32],
    l2_max: &[f32],
    ax: usize,
    ay: usize,
    az: usize,
    interior: usize,
    threshold: f32,
) -> SeamStats {
    let texels = interior * interior;
    let mut stats = SeamStats::default();
    let mut res_sum = 0.0f64;
    let mut res_n = 0u64;
    let mut raw_sum = 0.0f64;
    let mut raw_n = 0u64;
    let mut cl_sum = 0.0f64;
    let mut cl_n = 0u64;

    let lin = |x: usize, y: usize, z: usize| x + y * ax + z * ax * ay;
    // Face pairs: for each brick, its +x/+y/+z neighbor.
    for z in 0..az {
        for y in 0..ay {
            for x in 0..ax {
                let a = lin(x, y, z);
                if !nonempty[a] || !bricks[a].nonempty {
                    continue;
                }
                let la = choose_level(l1_max[a], l2_max[a], threshold);
                // axis 0: +x neighbor. A's +x face (face 0) vs B's -x face (face 3).
                let neighbors = [
                    (0usize, 3usize, x + 1 < ax, lin((x + 1).min(ax - 1), y, z)),
                    (1, 4, y + 1 < ay, lin(x, (y + 1).min(ay - 1), z)),
                    (2, 5, z + 1 < az, lin(x, y, (z + 1).min(az - 1))),
                ];
                for (fa, fb, exists, b) in neighbors {
                    if !exists || !nonempty[b] || !bricks[b].nonempty {
                        continue;
                    }
                    let lb = choose_level(l1_max[b], l2_max[b], threshold);
                    let face_a = &bricks[a].faces[fa];
                    let face_b = &bricks[b].faces[fb];
                    if face_a.len() != AF * AF || face_b.len() != AF * AF {
                        continue;
                    }
                    stats.pairs += 1;
                    let cross_level = la as u8 != lb as u8;
                    if cross_level {
                        stats.cross_level_pairs += 1;
                    }
                    // Pair boundary positions by their two in-face coordinates.
                    for p in 0..AF * AF {
                        let pa = &face_a[p];
                        let pb = &face_b[p];
                        if !pa.valid || !pb.valid {
                            continue;
                        }
                        let (resid_a, recon_a) = residual_for_level(pa, la);
                        let (resid_b, recon_b) = residual_for_level(pb, lb);
                        for texel in 0..texels {
                            let seam = (resid_a[texel] - resid_b[texel]).abs().max_element();
                            let raw = (recon_a[texel] - recon_b[texel]).abs().max_element();
                            stats.residual_max = stats.residual_max.max(seam);
                            stats.raw_max = stats.raw_max.max(raw);
                            res_sum += seam as f64;
                            res_n += 1;
                            raw_sum += raw as f64;
                            raw_n += 1;
                            if cross_level {
                                stats.cross_level_residual_max =
                                    stats.cross_level_residual_max.max(seam);
                                cl_sum += seam as f64;
                                cl_n += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    stats.residual_mean = if res_n > 0 { (res_sum / res_n as f64) as f32 } else { 0.0 };
    stats.raw_mean = if raw_n > 0 { (raw_sum / raw_n as f64) as f32 } else { 0.0 };
    stats.cross_level_residual_mean =
        if cl_n > 0 { (cl_sum / cl_n as f64) as f32 } else { 0.0 };
    stats
}

/// Residual and reconstructed value at a boundary position for a chosen level.
/// L0 reconstructs exactly (residual 0, recon = truth); we reconstruct truth by
/// residual+recon consistency: for L0 the residual is zero and the reconstructed
/// value equals truth, which we recover as `l1_recon - l1_residual` (= truth).
fn residual_for_level<'a>(pos: &'a FacePos, level: Level) -> (Vec<Vec3>, Vec<Vec3>) {
    match level {
        Level::L0 => {
            // truth = l1_recon - l1_residual (exact); residual zero.
            let truth: Vec<Vec3> = pos
                .l1_recon
                .iter()
                .zip(&pos.l1_residual)
                .map(|(r, res)| *r - *res)
                .collect();
            (vec![Vec3::ZERO; truth.len()], truth)
        }
        Level::L1 => (pos.l1_residual.clone(), pos.l1_recon.clone()),
        Level::L2 => (pos.l2_residual.clone(), pos.l2_recon.clone()),
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

pub fn write_json(report: &AnalysisReport, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(report)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn log_summary(report: &AnalysisReport) {
    log::info!(
        "[sh-analyze] grid {}x{}x{} = {} probes ({} valid), {} bricks ({} non-empty, {} protected); \
         base_direct={} deltas[id27={} id41={} id45={}]",
        report.grid_dims[0],
        report.grid_dims[1],
        report.grid_dims[2],
        report.total_probes,
        report.valid_probes,
        report.brick_count,
        report.nonempty_bricks,
        report.protected_bricks,
        report.has_base_direct,
        report.has_delta_indirect,
        report.has_delta_direct,
        report.has_delta_anim_direct,
    );
    let agg = |name: &str, a: &AggregateErr| {
        log::info!(
            "[sh-analyze] {name}: max {:.5} mean {:.5} p95(brick-mean) {:.5} weighted-mean {:.5} over {} bricks",
            a.max,
            a.mean,
            a.p95_of_brick_means,
            a.weighted_mean,
            a.bricks,
        );
    };
    log::info!("[sh-analyze] === base-irradiance reconstruction error (metric 1) ===");
    agg("base L1", &report.base_l1_aggregate);
    agg("base L2", &report.base_l2_aggregate);
    log::info!("[sh-analyze] === composed-receiver error (metric 2, PRIMARY) ===");
    agg("composed L1 (unweighted)", &report.composed_l1_aggregate);
    agg("composed L2 (unweighted)", &report.composed_l2_aggregate);
    agg("composed L1 (cosine-weighted)", &report.composed_l1_weighted_aggregate);
    agg("composed L2 (cosine-weighted)", &report.composed_l2_weighted_aggregate);

    log::info!("[sh-analyze] === shared-face seam error (metric 3) ===");
    log::info!(
        "[sh-analyze] seams: {} pairs ({} cross-level); residual-diff max {:.5} mean {:.5}; \
         raw-diff max {:.5} mean {:.5}; cross-level residual max {:.5} mean {:.5}",
        report.seam.pairs,
        report.seam.cross_level_pairs,
        report.seam.residual_max,
        report.seam.residual_mean,
        report.seam.raw_max,
        report.seam.raw_mean,
        report.seam.cross_level_residual_max,
        report.seam.cross_level_residual_mean,
    );

    log::info!("[sh-analyze] === byte accounting (three independent lines) ===");
    for s in &report.section_bytes {
        log::info!(
            "[sh-analyze] id {}: uniform {} B | (a)compacted {} B ({:.3}) | (b)exact-zero-drop {} B | (c)all-L1 {} B ({:.3}) all-L2 {} B ({:.3})",
            s.id,
            s.uniform_bytes,
            s.compacted_bytes,
            s.compacted_ratio,
            s.exact_zero_dropped_bytes,
            s.coarsen_all_l1_bytes,
            s.coarsen_all_l1_ratio,
            s.coarsen_all_l2_bytes,
            s.coarsen_all_l2_ratio,
        );
    }
    let c = &report.composed_atlas;
    log::info!(
        "[sh-analyze] composed-atlas: uniform {} B | compacted {} B ({:.3}) | all-L1 {} B ({:.3}) all-L2 {} B ({:.3})",
        c.uniform_bytes,
        c.compacted_bytes,
        c.compacted_ratio,
        c.coarsen_all_l1_bytes,
        c.coarsen_all_l1_ratio,
        c.coarsen_all_l2_bytes,
        c.coarsen_all_l2_ratio,
    );
    log::info!(
        "[sh-analyze] delta entries {} — exact-zero fraction {:.4} (drop oracle), near-zero(<{:.0e}) fraction {:.4}",
        report.total_delta_entries,
        report.exact_zero_entry_fraction,
        report.near_zero_eps,
        report.near_zero_entry_fraction,
    );

    log::info!("[sh-analyze] === threshold sweep (composed-error gate; L0/L1/L2 histogram + projected savings) ===");
    for row in &report.sweep {
        log::info!(
            "[sh-analyze] t={:.4}: L0/L1/L2 {}/{}/{} | proj {} B ({:.3}x) | +protect L0/L1/L2 {}/{}/{} proj {} B ({:.3}x)",
            row.threshold,
            row.l0,
            row.l1,
            row.l2,
            row.projected_bytes,
            row.ratio_to_uniform,
            row.l0_protected,
            row.l1_protected,
            row.l2_protected,
            row.projected_bytes_protected,
            row.ratio_to_uniform_protected,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn const_tiles(value: f32, texels: usize) -> [Option<Tile>; PROBES_PER_CELL] {
        std::array::from_fn(|_| Some(vec![Vec3::splat(value); texels]))
    }

    #[test]
    fn l2_mean_of_constant_brick_is_exact() {
        let texels = 16;
        let tiles = const_tiles(3.0, texels);
        let recon = reconstruct_l2_tile(&tiles, texels).unwrap();
        for v in &recon {
            assert!((v.x - 3.0).abs() < 1e-6);
        }
        // Error against any probe is zero.
        let err = level_errors(&tiles, LevelKind::L2, texels, 4, &vec![1.0; texels]);
        assert!(err.max < 1e-6, "constant brick L2 error must be ~0");
    }

    #[test]
    fn l1_trilinear_reproduces_linear_ramp_exactly() {
        // Build a brick whose per-probe value is a linear function of local x.
        // Trilinear from the 8 corners must reproduce it exactly.
        let texels = 4;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        for local in 0..PROBES_PER_CELL {
            let (lx, _ly, _lz) = local_xyz(local);
            let val = 10.0 + lx as f32 * 2.0; // linear in x only
            tiles[local] = Some(vec![Vec3::splat(val); texels]);
        }
        for target in 0..PROBES_PER_CELL {
            let recon = reconstruct_l1_tile(&tiles, target, texels).unwrap();
            let (lx, _, _) = local_xyz(target);
            let expect = 10.0 + lx as f32 * 2.0;
            assert!(
                (recon[0].x - expect).abs() < 1e-4,
                "L1 trilinear must reproduce a linear ramp: got {} expect {expect}",
                recon[0].x
            );
        }
        let err = level_errors(&tiles, LevelKind::L1, texels, 2, &vec![1.0; texels]);
        assert!(err.max < 1e-4, "linear ramp L1 error must be ~0, got {}", err.max);
    }

    #[test]
    fn l1_weight_endpoints() {
        // Corner (0,0,0) at target (0,0,0) → weight 1.
        assert!((trilinear_weight((0, 0, 0), (0, 0, 0)) - 1.0).abs() < 1e-6);
        // Corner (3,3,3) at target (3,3,3) → weight 1.
        assert!((trilinear_weight((3, 3, 3), (3, 3, 3)) - 1.0).abs() < 1e-6);
        // Corner (3,0,0) at target (0,0,0) → weight 0.
        assert!(trilinear_weight((0, 0, 0), (3, 0, 0)).abs() < 1e-6);
        // Midpoint target (weights sum to 1 across corners).
        let mut sum = 0.0;
        for c in corner_locals() {
            sum += trilinear_weight((1, 1, 1), local_xyz(c));
        }
        assert!((sum - 1.0).abs() < 1e-5, "trilinear weights must partition unity");
    }

    #[test]
    fn seam_residual_zero_when_both_bricks_l0() {
        // Two adjacent constant bricks reconstructed at L0 have zero residual, so
        // the seam residual-difference is zero regardless of their true values.
        let texels = 4;
        let bt_a = BrickTiles {
            composed: const_tiles(1.0, texels),
            base: const_tiles(1.0, texels),
            valid_mask: [true; PROBES_PER_CELL],
            in_bounds: [true; PROBES_PER_CELL],
        };
        let bt_b = BrickTiles {
            composed: const_tiles(5.0, texels),
            base: const_tiles(5.0, texels),
            valid_mask: [true; PROBES_PER_CELL],
            in_bounds: [true; PROBES_PER_CELL],
        };
        let sa = build_seam_brick(&bt_a, texels, 2, &vec![1.0; texels]);
        let sb = build_seam_brick(&bt_b, texels, 2, &vec![1.0; texels]);
        // A's +x face (0) vs B's -x face (3), both L0.
        let face_a = &sa.faces[0];
        let face_b = &sb.faces[3];
        for p in 0..AF * AF {
            let (resid_a, _) = residual_for_level(&face_a[p], Level::L0);
            let (resid_b, _) = residual_for_level(&face_b[p], Level::L0);
            for t in 0..texels {
                let seam = (resid_a[t] - resid_b[t]).abs().max_element();
                assert!(seam < 1e-5, "L0-L0 seam residual must be ~0");
            }
        }
    }

    #[test]
    fn seam_residual_nonzero_across_l0_l2_on_gradient() {
        // Brick A: linear ramp (L0 exact → residual 0). Brick B: linear ramp,
        // reconstructed at L2 (mean → residual non-zero). The cross-level seam
        // must be non-zero.
        let texels = 1;
        let mut a: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        let mut b: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        for local in 0..PROBES_PER_CELL {
            let (lx, _, _) = local_xyz(local);
            a[local] = Some(vec![Vec3::splat(lx as f32); texels]);
            b[local] = Some(vec![Vec3::splat(lx as f32); texels]);
        }
        let bt_a = BrickTiles {
            composed: a,
            base: std::array::from_fn(|_| None),
            valid_mask: [true; PROBES_PER_CELL],
            in_bounds: [true; PROBES_PER_CELL],
        };
        let bt_b = BrickTiles {
            composed: b,
            base: std::array::from_fn(|_| None),
            valid_mask: [true; PROBES_PER_CELL],
            in_bounds: [true; PROBES_PER_CELL],
        };
        let sa = build_seam_brick(&bt_a, texels, 2, &vec![1.0; texels]);
        let sb = build_seam_brick(&bt_b, texels, 2, &vec![1.0; texels]);
        let face_a = &sa.faces[0];
        let face_b = &sb.faces[3];
        let mut max_seam = 0.0f32;
        for p in 0..AF * AF {
            let (resid_a, _) = residual_for_level(&face_a[p], Level::L0);
            let (resid_b, _) = residual_for_level(&face_b[p], Level::L2);
            for t in 0..texels {
                let seam = (resid_a[t] - resid_b[t]).abs().max_element();
                max_seam = max_seam.max(seam);
            }
        }
        assert!(max_seam > 0.1, "L0|L2 seam on a gradient must be non-zero, got {max_seam}");
    }
}
