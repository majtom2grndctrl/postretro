//! Output-preserving SH coarsenability analysis pass (spike, measurement only).
//!
//! Governing intent: `context/research/archived-plans/lighting-scale--adaptive-base-probe-density`
//! (design intent ONLY — its surface-distance classifier and seam proxy are
//! ABANDONED). This module classifies coarsenability from **composed receiver
//! error**, measures seams as **actual shared-face reconstruction differences**,
//! attributes valid-probe payload compaction / exact-zero delta dropping / density coarsening
//! **separately**, and reports savings both with and without a protection
//! stand-in.
//!
//! **Scope note (delta-SH probe coarsening).** This module's own analysis entry
//! point (`run_analysis`) still never touches an emitted `.prl` byte. But
//! several of its brick-assembly / error primitives — `build_brick_tiles`,
//! `level_errors`, `tile_magnitude`, `accumulate_delta_for_cell`,
//! `brick_world_aabb`, `DeltaView`, `AnalyzeInputs`, `LevelKind` — are now
//! `pub(crate)` and reused by `sh_coarsen::classify_section_levels` to compute
//! per-cell coarsening levels, which **do** change emitted `delta_subblocks`
//! bytes on a `--sh-coarsen` bake. Treat those primitives as producer-facing.
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
use postretro_level_format::sh_reconstruct::{
    Level, Tile, local_xyz, reconstruct_l1_tile, reconstruct_l2_tile, stored_delta_tiles,
    stored_tiles, zero_tile,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use serde::Serialize;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::delta_sections::EmittedDeltaSectionRef;
use crate::sh_bake::f16_bits_to_f32;

const AF: usize = AFFINITY_FACTOR as usize; // 4
const PROBES_PER_CELL: usize = AF * AF * AF; // 64

/// A wider near-zero fraction is reported separately from the bit-zero entry
/// fraction at [`NEAR_ZERO_EPS`].
const NEAR_ZERO_EPS: f32 = 1.0e-4;

/// Default composed-error thresholds swept in the histogram/savings table.
/// Irradiance units; seeded to bracket the plausible coarsenability band from
/// near-lossless (0.005) up past the observed per-brick composed-L2 max on the
/// stress map (~3.4) so the histogram actually discriminates L0 vs L2.
pub const DEFAULT_THRESHOLDS: [f32; 11] =
    [0.005, 0.01, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

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
pub(crate) struct DeltaView<'a> {
    pub(crate) affinity_dims: [u32; 3],
    tile_dimension: usize,
    valid_probe_masks: &'a [u64],
    offsets: &'a [u32],
    subblocks: &'a [u16],
    /// Starting f16 offset for every CSR entry, in the final compact payload
    /// order. The trailing value is the total payload length.
    entry_payload_offsets: Vec<usize>,
    /// Id 41 must retain the final canonical record for each selected light so
    /// loader coverage survives even when that record is bit-zero.
    exact_zero_drop_exempt: Vec<bool>,
}

impl<'a> DeltaView<'a> {
    pub(crate) fn from_indirect(s: &'a DeltaShVolumesSection) -> Self {
        Self::new(
            s.affinity_dims,
            s.tile_dimension as usize,
            &s.valid_probe_masks,
            &s.affinity_offsets,
            &s.delta_subblocks,
        )
    }
    pub(crate) fn from_direct(s: &'a DirectShDeltaVolumesSection) -> Self {
        let mut view = Self::new(
            s.affinity_dims,
            s.tile_dimension as usize,
            &s.valid_probe_masks,
            &s.affinity_offsets,
            &s.delta_subblocks,
        );
        view.exact_zero_drop_exempt = vec![false; s.affinity_lights.len()];
        let mut final_entry_for_light = std::collections::BTreeMap::new();
        for (entry, &light) in s.affinity_lights.iter().enumerate() {
            final_entry_for_light.insert(light, entry);
        }
        for entry in final_entry_for_light.into_values() {
            view.exact_zero_drop_exempt[entry] = true;
        }
        view
    }
    pub(crate) fn from_anim_direct(s: &'a AnimatedDirectShDeltaVolumesSection) -> Self {
        Self::new(
            s.affinity_dims,
            s.tile_dimension as usize,
            &s.valid_probe_masks,
            &s.affinity_offsets,
            &s.delta_subblocks,
        )
    }

    fn new(
        affinity_dims: [u32; 3],
        tile_dimension: usize,
        valid_probe_masks: &'a [u64],
        offsets: &'a [u32],
        subblocks: &'a [u16],
    ) -> Self {
        let probe_f16_stride = tile_dimension * tile_dimension * 4;
        let mut entry_payload_offsets = Vec::new();
        let mut payload_offset = 0usize;
        for (offsets, &valid_probe_mask) in offsets.windows(2).zip(valid_probe_masks) {
            let entry_count = offsets[1].saturating_sub(offsets[0]) as usize;
            let entry_f16_count = valid_probe_mask.count_ones() as usize * probe_f16_stride;
            for _ in 0..entry_count {
                entry_payload_offsets.push(payload_offset);
                payload_offset = payload_offset.saturating_add(entry_f16_count);
            }
        }
        entry_payload_offsets.push(payload_offset);

        Self {
            affinity_dims,
            tile_dimension,
            valid_probe_masks,
            offsets,
            subblocks,
            entry_payload_offsets,
            exact_zero_drop_exempt: Vec::new(),
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
    fn entry_payload_range(&self, entry: usize) -> Option<std::ops::Range<usize>> {
        let start = *self.entry_payload_offsets.get(entry)?;
        let end = *self.entry_payload_offsets.get(entry + 1)?;
        (end <= self.subblocks.len()).then_some(start..end)
    }
    fn entry_payload_f16_count(&self, entry: usize) -> usize {
        self.entry_payload_range(entry)
            .map_or(0, |range| range.len())
    }
    pub(crate) fn valid_probe_mask(&self, cell: usize) -> Option<u64> {
        self.valid_probe_masks.get(cell).copied()
    }
    fn is_exact_zero_drop_candidate(&self, entry: usize, exact_zero: bool) -> bool {
        exact_zero
            && !self
                .exact_zero_drop_exempt
                .get(entry)
                .copied()
                .unwrap_or(false)
    }
    /// Resolve a probe tile through the compact section's per-cell validity
    /// descriptor and its entry-order payload prefix. Invalid probes have no
    /// stored tile; valid probes use their x-fastest rank among set mask bits.
    fn resolve_probe_f16_offset(&self, cell: usize, entry: usize, local: usize) -> Option<usize> {
        if local >= PROBES_PER_CELL {
            return None;
        }
        let entry_start = *self.offsets.get(cell)? as usize;
        let entry_end = *self.offsets.get(cell + 1)? as usize;
        if !(entry_start..entry_end).contains(&entry) {
            return None;
        }
        let valid_probe_mask = self.valid_probe_mask(cell)?;
        let local_bit = 1u64 << local;
        if valid_probe_mask & local_bit == 0 {
            return None;
        }
        let within_cell_rank = (valid_probe_mask & (local_bit - 1)).count_ones() as usize;
        self.entry_payload_range(entry)?
            .start
            .checked_add(within_cell_rank.checked_mul(self.probe_f16_stride())?)
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

/// Owned by the pipeline until final compaction completes, then borrowed by the
/// emitted-vs-dense diagnostic. These are the post-drop, still-dense sections;
/// they never reach the packer.
pub(crate) struct DenseDeltaSections<'a> {
    pub indirect: Option<&'a DeltaShVolumesSection>,
    pub direct: Option<&'a DirectShDeltaVolumesSection>,
    pub animated_direct: Option<&'a AnimatedDirectShDeltaVolumesSection>,
}

/// Dense, post-exact-zero-drop delta payload retained only by the diagnostic
/// path. Unlike [`DeltaView`], this is deliberately pre-compaction: every CSR
/// entry has a fixed 64-probe payload, irrespective of its later cell level.
struct DenseDeltaView<'a> {
    affinity_dims: [u32; 3],
    offsets: &'a [u32],
    subblocks: &'a [u16],
    tile_dimension: usize,
    tile_border: usize,
}

impl<'a> DenseDeltaView<'a> {
    fn from_indirect(section: &'a DeltaShVolumesSection) -> Self {
        Self::new(
            section.affinity_dims,
            &section.affinity_offsets,
            &section.delta_subblocks,
            section.tile_dimension,
            section.tile_border,
        )
    }

    fn from_direct(section: &'a DirectShDeltaVolumesSection) -> Self {
        Self::new(
            section.affinity_dims,
            &section.affinity_offsets,
            &section.delta_subblocks,
            section.tile_dimension,
            section.tile_border,
        )
    }

    fn from_anim_direct(section: &'a AnimatedDirectShDeltaVolumesSection) -> Self {
        Self::new(
            section.affinity_dims,
            &section.affinity_offsets,
            &section.delta_subblocks,
            section.tile_dimension,
            section.tile_border,
        )
    }

    fn new(
        affinity_dims: [u32; 3],
        offsets: &'a [u32],
        subblocks: &'a [u16],
        tile_dimension: u32,
        tile_border: u32,
    ) -> Self {
        Self {
            affinity_dims,
            offsets,
            subblocks,
            tile_dimension: tile_dimension as usize,
            tile_border: tile_border as usize,
        }
    }

    fn probe_stride(&self) -> usize {
        self.tile_dimension * self.tile_dimension * 4
    }

    fn interior_texels(&self) -> usize {
        let edge = self.tile_dimension.saturating_sub(self.tile_border * 2);
        edge * edge
    }

    fn entry_range(&self, cell: usize) -> Option<std::ops::Range<usize>> {
        Some(*self.offsets.get(cell)? as usize..*self.offsets.get(cell + 1)? as usize)
    }

    fn decode_entry_local(&self, entry: usize, local: usize) -> anyhow::Result<Tile> {
        anyhow::ensure!(
            local < PROBES_PER_CELL,
            "dense delta local {local} out of range"
        );
        let start = entry
            .checked_mul(PROBES_PER_CELL)
            .and_then(|n| n.checked_mul(self.probe_stride()))
            .and_then(|n| n.checked_add(local.checked_mul(self.probe_stride())?))
            .ok_or_else(|| anyhow::anyhow!("dense delta tile offset overflow"))?;
        let end = start
            .checked_add(self.probe_stride())
            .ok_or_else(|| anyhow::anyhow!("dense delta tile end overflow"))?;
        anyhow::ensure!(
            end <= self.subblocks.len(),
            "dense delta entry {entry} local {local} exceeds {}-f16 payload",
            self.subblocks.len()
        );
        let mut tile = Vec::with_capacity(self.interior_texels());
        for y in self.tile_border..self.tile_dimension - self.tile_border {
            for x in self.tile_border..self.tile_dimension - self.tile_border {
                let i = start + (y * self.tile_dimension + x) * 4;
                tile.push(Vec3::new(
                    f16_bits_to_f32(self.subblocks[i]),
                    f16_bits_to_f32(self.subblocks[i + 1]),
                    f16_bits_to_f32(self.subblocks[i + 2]),
                ));
            }
        }
        Ok(tile)
    }
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

// Reconstruction math (`corner_locals` / `local_xyz` / `trilinear_weight` /
// `reconstruct_l1_tile` / `reconstruct_l2_tile`) now lives in the shared
// `postretro_level_format::sh_reconstruct` module, imported above.

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
    // NaN-tolerant: a corrupt/garbage atlas texel can decode to NaN, and under
    // `--sh-coarsen` this stat gates a real bake — treat NaN as equal rather
    // than panicking the sort (mirrors the classifier's own guarded sort).
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((v.len() as f32 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn weighted_percentile(values: &[f32], weights: &[f32], p: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut pairs: Vec<(f32, f32)> = values
        .iter()
        .copied()
        .zip(weights.iter().copied())
        .collect();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
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
    /// False when the level could not be reconstructed at all (e.g. L1 when the
    /// brick's 8 corner probes are all invalid). An unevaluable level is
    /// INELIGIBLE for coarsening — never treated as error-zero by the gate.
    pub evaluable: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct MagnitudeStats {
    /// Max over interior texels of max-per-channel `|truth|` (linear irradiance).
    pub max: f32,
    /// Texel-count mean of max-per-channel `|truth|`.
    pub mean: f32,
    /// 95th percentile of per-texel max-per-channel `|truth|`.
    pub p95: f32,
    /// Interior texels sampled across the brick's valid probe tiles.
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
    /// Composed-receiver irradiance magnitude over the brick's valid probe
    /// tiles, measured with the same max-per-channel reduction `texel_error`
    /// uses. `composed_l*.{stat}` divided by the matching magnitude is a
    /// like-for-like relative (Weber) deviation, which the raw absolute-error
    /// metric alone cannot express — this is what lets a metric-only threshold
    /// be stated as a percentage of local brightness rather than an absolute
    /// irradiance value with no perceptual anchor.
    pub composed_magnitude: MagnitudeStats,
    pub world_min: [f32; 3],
    pub world_max: [f32; 3],
}

/// Actual emitted reconstruction measured against the dense, post-drop delta
/// truth. This is deliberately distinct from [`BrickRecord`], whose L1/L2
/// fields remain the analysis sweep's candidate reconstructions.
#[derive(Serialize, Clone, Default)]
pub struct EmittedBrickRecord {
    pub cell: [u32; 3],
    pub linear_cell: u32,
    /// Dense composed truth. Never derived from the coarsened payload.
    pub dense_truth_magnitude: MagnitudeStats,
    /// Error of the final serialized L0/L1/L2 sections against that dense truth.
    pub emitted_error: LevelErrStats,
    pub relative_p95: f32,
    pub relative_max: f32,
}

#[derive(Serialize, Clone, Default)]
pub struct EmittedReconstructionReport {
    /// The classifier's truncated-order-statistic map p95, calculated from
    /// `dense_truth_magnitude.p95` over non-empty bricks.
    pub dense_truth_map_p95: f32,
    pub darkness_floor: f32,
    pub rel_p95_limit: f32,
    pub rel_max_limit: f32,
    pub failing_bricks: u64,
    pub bricks: Vec<EmittedBrickRecord>,
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
    /// Line (a): after valid-probe payload compaction.
    pub compacted_bytes: u64,
    /// Line (b): bytes recoverable by dropping eligible exact-zero delta
    /// entries (delta sections only; 0 for id 34). Id 41 coverage fallbacks are
    /// not eligible.
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

    /// Bit-zero delta entry fraction in the finalized sections, aggregated
    /// across all three sections. Coverage fallbacks are included.
    pub exact_zero_entry_fraction: f32,
    pub near_zero_entry_fraction: f32,
    pub near_zero_eps: f32,
    pub total_delta_entries: u64,

    pub seam: SeamStats,
    pub sweep: Vec<SweepRow>,

    pub protect_aabbs: Vec<[f32; 6]>,

    pub bricks: Vec<BrickRecord>,
    /// Present only when a coarsened bake retained its dense post-drop source
    /// long enough to compare it with the finalized serialized payload.
    pub emitted_reconstruction: Option<EmittedReconstructionReport>,
}

// ---------------------------------------------------------------------------
// Byte model
// ---------------------------------------------------------------------------

// `Level`, `stored_tiles`, and `stored_delta_tiles` now live in the shared
// `postretro_level_format::sh_reconstruct` module, imported above.

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
pub(crate) fn accumulate_delta_for_cell(
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
    let start = view.offsets[cell] as usize;
    let end = view.offsets[cell + 1] as usize;
    for entry in start..end {
        for (local, accumulated_tile) in acc.iter_mut().enumerate() {
            let Some(probe_base) = view.resolve_probe_f16_offset(cell, entry, local) else {
                continue;
            };
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
                    accumulated_tile[iy * interior + ix] += Vec3::new(r, g, b);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-brick working set
// ---------------------------------------------------------------------------

pub(crate) struct BrickTiles {
    /// Composed truth tile per local probe (Some iff in-bounds AND valid).
    pub(crate) composed: [Option<Tile>; PROBES_PER_CELL],
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
    let ax = nx.div_ceil(AF);
    let ay = ny.div_ceil(AF);
    let az = nz.div_ceil(AF);
    let brick_count = ax * ay * az;
    report.brick_count = brick_count as u64;

    let expected_affinity = [ax as u32, ay as u32, az as u32];
    let delta_indirect = check_delta(
        inputs.delta_indirect.map(DeltaView::from_indirect),
        expected_affinity,
        "id27",
    );
    let delta_direct = check_delta(
        inputs.delta_direct.map(DeltaView::from_direct),
        expected_affinity,
        "id41",
    );
    let delta_anim = check_delta(
        inputs.delta_anim_direct.map(DeltaView::from_anim_direct),
        expected_affinity,
        "id45",
    );

    // Aggregate accumulators.
    let mut base_l1_agg = AggAcc::default();
    let mut base_l2_agg = AggAcc::default();
    let mut comp_l1_agg = AggAcc::default();
    let mut comp_l2_agg = AggAcc::default();
    let mut comp_l1_w_agg = AggAcc::default();
    let mut comp_l2_w_agg = AggAcc::default();

    // Sweep counters.
    let thresholds = inputs.thresholds;

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

    // Delta uniform/compacted/coarsen accumulators (aggregate across 3 sections).
    let mut delta_uniform_tiles = 0u64; // entries * 64
    let mut delta_compacted_tiles = 0u64; // entries * valid-in-cell
    let mut delta_l1_tiles = 0u64;
    let mut delta_l2_tiles = 0u64;
    let mut delta_exact_zero_entries = 0u64;
    let mut delta_near_zero_entries = 0u64;
    let mut delta_total_entries = 0u64;

    // Per-section entry totals for the exact-zero candidate byte line.
    let mut per_section_entries: [u64; 3] = [0; 3];
    let mut per_section_exact_zero_candidates: [u64; 3] = [0; 3];

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
    let mut brick_valid_masks: Vec<[bool; PROBES_PER_CELL]> =
        vec![[false; PROBES_PER_CELL]; brick_count];
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
                    inputs,
                    base,
                    tile_dim,
                    interior,
                    border,
                    &valid_rank,
                    dims,
                    cell_lin,
                    cx,
                    cy,
                    cz,
                    ax,
                    ay,
                    &delta_indirect,
                    &delta_direct,
                    &delta_anim,
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
                let comp_magnitude = tile_magnitude(&bt.composed, texels);

                base_l1_agg.push(&base_l1);
                base_l2_agg.push(&base_l2);
                comp_l1_agg.push(&comp_l1);
                comp_l2_agg.push(&comp_l2);
                comp_l1_w_agg.push_weighted(&comp_l1);
                comp_l2_w_agg.push_weighted(&comp_l2);

                brick_comp_l1_max[cell_lin] = comp_l1.gate_max();
                brick_comp_l2_max[cell_lin] = comp_l2.gate_max();

                // --- Byte lines (base) ---
                base_uniform_tiles += PROBES_PER_CELL as u64; // dense brick
                base_compacted_tiles += stored_tiles(Level::L0, &bt.valid_mask) as u64;
                base_l1_tiles += stored_tiles(Level::L1, &bt.valid_mask) as u64;
                base_l2_tiles += stored_tiles(Level::L2, &bt.valid_mask) as u64;

                // --- Seam cache (boundary layers) ---
                seam_bricks[cell_lin] = build_seam_brick(&bt, texels, interior, &weights);

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
                    composed_magnitude: comp_magnitude,
                    world_min: [wmin.x, wmin.y, wmin.z],
                    world_max: [wmax.x, wmax.y, wmax.z],
                });
            }
        }
    }

    // --- Delta byte lines + exact-zero classification (aggregate over 3 sections) ---
    for (si, view) in [&delta_indirect, &delta_direct, &delta_anim]
        .into_iter()
        .enumerate()
    {
        let Some(view) = view else { continue };
        let entries = view.entry_count() as u64;
        delta_total_entries += entries;
        per_section_entries[si] = entries;
        delta_uniform_tiles += entries * PROBES_PER_CELL as u64;

        // Walk each cell's finalized entries: exact-zero classification plus
        // per-level compacted stored tiles. The section descriptor is the
        // emitted compact payload identity, so do not infer this count from the
        // dense base-grid model.
        for c in 0..view.offsets.len().saturating_sub(1) {
            let valid_probe_mask = view.valid_probe_mask(c).unwrap_or(0);
            let l0 = stored_delta_tiles(Level::L0, valid_probe_mask) as u64;
            let l1 = stored_delta_tiles(Level::L1, valid_probe_mask) as u64;
            let l2 = stored_delta_tiles(Level::L2, valid_probe_mask) as u64;
            let start = view.offsets[c] as usize;
            let end = view.offsets[c + 1] as usize;
            for entry in start..end {
                let (exact_zero, near_zero) = entry_zeroness(view, entry);
                let exact_zero_drop_candidate =
                    view.is_exact_zero_drop_candidate(entry, exact_zero);
                if exact_zero {
                    delta_exact_zero_entries += 1;
                }
                if exact_zero_drop_candidate {
                    per_section_exact_zero_candidates[si] += 1;
                }
                if near_zero {
                    delta_near_zero_entries += 1;
                }
                // Density coarsening is independent from exact-zero dropping:
                // project every entry present in the finalized section.
                delta_compacted_tiles += l0;
                delta_l1_tiles += l1;
                delta_l2_tiles += l2;
                brick_delta_l0_tiles[c] += l0;
                brick_delta_l1_tiles[c] += l1;
                brick_delta_l2_tiles[c] += l2;
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
    // Delta sections preserve a dense-64 baseline for counterfactual reports,
    // but line (a) is the actual compact emitted payload. Its length must come
    // from the final variable-stride section rather than a base-grid estimate.
    let delta_ids = [
        (27u32, &delta_indirect),
        (41, &delta_direct),
        (45, &delta_anim),
    ];
    for (id, view) in delta_ids {
        let Some(view) = view else { continue };
        let uniform_f16_count = view.entry_count().saturating_mul(view.subblock_stride());
        let mut l1 = 0u64;
        let mut l2 = 0u64;
        let mut exact_zero_f16_count = 0usize;
        for c in 0..view.offsets.len().saturating_sub(1) {
            let valid_probe_mask = view.valid_probe_mask(c).unwrap_or(0);
            let sl1 = stored_delta_tiles(Level::L1, valid_probe_mask) as u64;
            let sl2 = stored_delta_tiles(Level::L2, valid_probe_mask) as u64;
            let start = view.offsets[c] as usize;
            let end = view.offsets[c + 1] as usize;
            for entry in start..end {
                let (exact_zero, _) = entry_zeroness(view, entry);
                if view.is_exact_zero_drop_candidate(entry, exact_zero) {
                    exact_zero_f16_count += view.entry_payload_f16_count(entry);
                }
                l1 += sl1;
                l2 += sl2;
            }
        }
        let uniform_bytes = uniform_f16_count as u64 * 2;
        let compacted_bytes = view.subblocks.len() as u64 * 2;
        let exact_zero_dropped_bytes = exact_zero_f16_count as u64 * 2;
        let coarsen_all_l1_bytes = l1 * view.probe_f16_stride() as u64 * 2;
        let coarsen_all_l2_bytes = l2 * view.probe_f16_stride() as u64 * 2;
        report.section_bytes.push(SectionBytes {
            id,
            uniform_bytes,
            compacted_bytes,
            exact_zero_dropped_bytes,
            coarsen_all_l1_bytes,
            coarsen_all_l2_bytes,
            compacted_ratio: ratio(compacted_bytes, uniform_bytes),
            coarsen_all_l1_ratio: ratio(coarsen_all_l1_bytes, uniform_bytes),
            coarsen_all_l2_ratio: ratio(coarsen_all_l2_bytes, uniform_bytes),
        });
    }
    let _ = (
        delta_uniform_tiles,
        delta_compacted_tiles,
        delta_l1_tiles,
        delta_l2_tiles,
        per_section_entries,
        per_section_exact_zero_candidates,
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
    for &t in thresholds.iter() {
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
pub(crate) fn build_brick_tiles(
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
    let mut delta_acc: [Tile; PROBES_PER_CELL] = std::array::from_fn(|_| zero_tile(texels));
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
        let Some(base_ind_tile) = base_ind else {
            continue;
        };

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

pub(crate) fn brick_world_aabb(
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
pub(crate) enum LevelKind {
    L1,
    L2,
}

pub(crate) struct LevelErr {
    pub(crate) max: f32,
    mean: f32,
    pub(crate) p95: f32,
    weighted_mean: f32,
    weighted_p95: f32,
    pub(crate) texel_samples: u64,
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
            evaluable: self.texel_samples > 0,
        }
    }
    /// Gate value for level selection: the reconstruction max error when the
    /// level is evaluable, else +inf so the gate never selects it. Never
    /// serialized (JSON rejects non-finite floats).
    fn gate_max(&self) -> f32 {
        if self.texel_samples > 0 {
            self.max
        } else {
            f32::INFINITY
        }
    }
}

pub(crate) fn level_errors(
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

/// Composed-receiver irradiance magnitude over a brick's valid probe tiles.
/// Uses the max-per-channel reduction `texel_error` applies to differences, so
/// `error / magnitude` compares like with like. Absent (invalid) probes are
/// skipped, exactly as `level_errors` skips them. Returns a zeroed record when
/// the brick has no valid probe tiles.
pub(crate) fn tile_magnitude(
    tiles: &[Option<Tile>; PROBES_PER_CELL],
    texels: usize,
) -> MagnitudeStats {
    let mut acc = ErrAccum::default();
    for tile in tiles.iter() {
        let Some(truth) = tile else { continue };
        for v in &truth[..texels] {
            let m = v.x.abs().max(v.y.abs()).max(v.z.abs());
            acc.push(m, 1.0);
        }
    }
    if acc.is_empty() {
        return MagnitudeStats::default();
    }
    MagnitudeStats {
        max: acc.max(),
        mean: acc.mean(),
        p95: acc.p95(),
        texel_samples: acc.values.len() as u64,
    }
}

fn emitted_error(
    truth: &[Option<Tile>; PROBES_PER_CELL],
    emitted: &[Option<Tile>; PROBES_PER_CELL],
    texels: usize,
) -> LevelErr {
    let mut acc = ErrAccum::default();
    for local in 0..PROBES_PER_CELL {
        let (Some(truth), Some(emitted)) = (&truth[local], &emitted[local]) else {
            continue;
        };
        for texel in 0..texels {
            acc.push(texel_error(&emitted[texel], &truth[texel]), 1.0);
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

fn add_tile(dst: &mut Tile, src: &Tile) {
    for (dst, src) in dst.iter_mut().zip(src) {
        *dst += *src;
    }
}

/// Classifier darkness floor. This intentionally does *not* use [`percentile`]:
/// the classifier's map-wide p95 is truncated while per-brick error p95 is
/// rounded.
fn classifier_darkness_floor(magnitudes: &[f32], darkness_frac: f32) -> (f32, f32) {
    let mut sorted = magnitudes.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let map_p95 = sorted
        .get(((sorted.len().saturating_sub(1)) as f32 * 0.95).floor() as usize)
        .copied()
        .unwrap_or(0.0);
    (map_p95, (darkness_frac * map_p95).max(1.0e-6))
}

/// Adds one section's dense post-drop truth and final serialized reconstruction
/// for a cell. The two inputs intentionally have different layouts: `dense`
/// is fixed 64-probe pre-compaction data, while `emitted` ranks only kept bits.
fn accumulate_dense_and_emitted_delta(
    dense: &DenseDeltaView<'_>,
    emitted: &EmittedDeltaSectionRef<'_>,
    cell: usize,
    valid_mask: &[bool; PROBES_PER_CELL],
    dense_acc: &mut [Tile; PROBES_PER_CELL],
    emitted_acc: &mut [Tile; PROBES_PER_CELL],
) -> anyhow::Result<()> {
    let Some(dense_entries) = dense.entry_range(cell) else {
        return Ok(());
    };
    let Some(emitted_entries) = emitted.entry_range(cell) else {
        return Ok(());
    };
    anyhow::ensure!(
        dense_entries == emitted_entries,
        "dense/emitted delta CSR ranges disagree for cell {cell}: {dense_entries:?} vs {emitted_entries:?}"
    );
    anyhow::ensure!(
        dense.interior_texels() == emitted.interior_texels(),
        "dense/emitted delta tile interiors disagree for cell {cell}"
    );
    let emitted_validity = emitted
        .valid_probe_mask(cell)
        .ok_or_else(|| anyhow::anyhow!("emitted delta validity is missing for cell {cell}"))?;
    for entry in dense_entries {
        for local in 0..PROBES_PER_CELL {
            if !valid_mask[local] {
                continue;
            }
            anyhow::ensure!(
                emitted_validity & (1u64 << local) != 0,
                "emitted delta validity disagrees with the base grid at cell {cell} local {local}"
            );
            let truth = dense.decode_entry_local(entry, local)?;
            add_tile(&mut dense_acc[local], &truth);
            if let Some(reconstructed) = emitted.reconstruct_entry_tile(cell, entry, local)? {
                add_tile(&mut emitted_acc[local], &reconstructed);
            }
        }
    }
    Ok(())
}

/// Exact post-compaction diagnostic. The old analysis sweep remains candidate
/// based; this function instead compares final L0/L1/L2 bytes with the dense
/// post-drop source that the final bytes replaced.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_emitted_reconstruction_analysis(
    inputs: &AnalyzeInputs<'_>,
    dense_indirect: Option<&DeltaShVolumesSection>,
    dense_direct: Option<&DirectShDeltaVolumesSection>,
    dense_anim_direct: Option<&AnimatedDirectShDeltaVolumesSection>,
    emitted_indirect: Option<&DeltaShVolumesSection>,
    emitted_direct: Option<&DirectShDeltaVolumesSection>,
    emitted_anim_direct: Option<&AnimatedDirectShDeltaVolumesSection>,
) -> anyhow::Result<EmittedReconstructionReport> {
    let base = inputs.base_indirect;
    let dims = inputs.grid_dims;
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
    let total_probes = nx * ny * nz;
    let tile_dim = base.tile_dimension as usize;
    let border = base.tile_border as usize;
    let interior = tile_dim.saturating_sub(2 * border);
    anyhow::ensure!(
        total_probes > 0 && interior > 0,
        "emitted SH analysis has no usable base grid"
    );
    let texels = interior * interior;
    let (ax, ay, az) = (nx.div_ceil(AF), ny.div_ceil(AF), nz.div_ceil(AF));
    let affinity_dims = [ax as u32, ay as u32, az as u32];
    let brick_count = ax * ay * az;

    let mut valid_rank = vec![-1i64; total_probes];
    let mut rank = 0i64;
    for (probe, slot) in valid_rank.iter_mut().enumerate() {
        if inputs.validity.get(probe).copied().unwrap_or(0) != 0 {
            *slot = rank;
            rank += 1;
        }
    }

    let dense_indirect = dense_indirect.map(DenseDeltaView::from_indirect);
    let dense_direct = dense_direct.map(DenseDeltaView::from_direct);
    let dense_anim = dense_anim_direct.map(DenseDeltaView::from_anim_direct);
    let emitted_indirect = emitted_indirect
        .map(EmittedDeltaSectionRef::from_indirect)
        .transpose()?;
    let emitted_direct = emitted_direct
        .map(EmittedDeltaSectionRef::from_direct)
        .transpose()?;
    let emitted_anim = emitted_anim_direct
        .map(EmittedDeltaSectionRef::from_animated_direct)
        .transpose()?;

    let pairs = [
        (dense_indirect.as_ref(), emitted_indirect.as_ref()),
        (dense_direct.as_ref(), emitted_direct.as_ref()),
        (dense_anim.as_ref(), emitted_anim.as_ref()),
    ];
    for (dense, emitted) in pairs {
        anyhow::ensure!(
            dense.is_some() == emitted.is_some(),
            "emitted SH analysis needs matching dense and finalized section presence"
        );
        if let (Some(dense), Some(emitted)) = (dense, emitted) {
            anyhow::ensure!(
                dense.affinity_dims == affinity_dims && emitted.cell_count() == brick_count,
                "emitted SH analysis delta affinity grid disagrees with base grid"
            );
        }
    }

    let mut records = Vec::new();
    for cz in 0..az {
        for cy in 0..ay {
            for cx in 0..ax {
                let cell = cx + cy * ax + cz * ax * ay;
                let mut valid_mask = [false; PROBES_PER_CELL];
                let mut base_tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
                for local in 0..PROBES_PER_CELL {
                    let (lx, ly, lz) = local_xyz(local);
                    let (px, py, pz) = (cx * AF + lx, cy * AF + ly, cz * AF + lz);
                    if px >= nx || py >= ny || pz >= nz {
                        continue;
                    }
                    let probe = px + py * nx + pz * nx * ny;
                    if valid_rank[probe] < 0 {
                        continue;
                    }
                    let Some(mut tile) =
                        decode_base_indirect_tile(base, valid_rank[probe], interior, border)
                    else {
                        continue;
                    };
                    if let Some(base_direct) = inputs.base_direct
                        && let Some(direct) =
                            decode_base_direct_tile(base_direct, probe, interior, border)
                    {
                        add_tile(&mut tile, &direct);
                    }
                    valid_mask[local] = true;
                    base_tiles[local] = Some(tile);
                }
                if !valid_mask.iter().any(|&v| v) {
                    continue;
                }

                let mut dense_delta: [Tile; PROBES_PER_CELL] =
                    std::array::from_fn(|_| zero_tile(texels));
                let mut emitted_delta: [Tile; PROBES_PER_CELL] =
                    std::array::from_fn(|_| zero_tile(texels));
                for (dense, emitted) in pairs {
                    if let (Some(dense), Some(emitted)) = (dense, emitted) {
                        accumulate_dense_and_emitted_delta(
                            dense,
                            emitted,
                            cell,
                            &valid_mask,
                            &mut dense_delta,
                            &mut emitted_delta,
                        )?;
                    }
                }

                let mut truth: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
                let mut emitted: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
                for local in 0..PROBES_PER_CELL {
                    let Some(base_tile) = &base_tiles[local] else {
                        continue;
                    };
                    let mut truth_tile = base_tile.clone();
                    let mut emitted_tile = base_tile.clone();
                    add_tile(&mut truth_tile, &dense_delta[local]);
                    add_tile(&mut emitted_tile, &emitted_delta[local]);
                    truth[local] = Some(truth_tile);
                    emitted[local] = Some(emitted_tile);
                }
                records.push(EmittedBrickRecord {
                    cell: [cx as u32, cy as u32, cz as u32],
                    linear_cell: cell as u32,
                    dense_truth_magnitude: tile_magnitude(&truth, texels),
                    emitted_error: emitted_error(&truth, &emitted, texels).to_stats(),
                    relative_p95: 0.0,
                    relative_max: 0.0,
                });
            }
        }
    }

    let map_magnitudes: Vec<f32> = records
        .iter()
        .map(|record| record.dense_truth_magnitude.p95)
        .collect();
    let params = crate::sh_coarsen::CoarsenParams::default();
    let (map_p95, floor) = classifier_darkness_floor(&map_magnitudes, params.darkness_frac);
    let mut failures = 0u64;
    for record in &mut records {
        record.relative_p95 =
            record.emitted_error.p95 / record.dense_truth_magnitude.p95.max(floor);
        record.relative_max =
            record.emitted_error.max / record.dense_truth_magnitude.max.max(floor);
        if record.relative_p95 > params.rel_p95_max || record.relative_max > params.rel_max_max {
            failures += 1;
        }
    }
    Ok(EmittedReconstructionReport {
        dense_truth_map_p95: map_p95,
        darkness_floor: floor,
        rel_p95_limit: params.rel_p95_max,
        rel_max_limit: params.rel_max_max,
        failing_bricks: failures,
        bricks: records,
    })
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

/// Returns (exact_zero, near_zero) for one compact delta CSR entry payload.
fn entry_zeroness(view: &DeltaView<'_>, entry: usize) -> (bool, bool) {
    let Some(range) = view.entry_payload_range(entry) else {
        return (false, false);
    };
    let mut all_zero = true;
    let mut max_abs = 0.0f32;
    for &h in &view.subblocks[range] {
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
                    let l1res: Vec<Vec3> = l1.iter().zip(&truth).map(|(r, t)| *r - *t).collect();
                    let l2res: Vec<Vec3> = l2.iter().zip(&truth).map(|(r, t)| *r - *t).collect();
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
    stats.residual_mean = if res_n > 0 {
        (res_sum / res_n as f64) as f32
    } else {
        0.0
    };
    stats.raw_mean = if raw_n > 0 {
        (raw_sum / raw_n as f64) as f32
    } else {
        0.0
    };
    stats.cross_level_residual_mean = if cl_n > 0 {
        (cl_sum / cl_n as f64) as f32
    } else {
        0.0
    };
    stats
}

/// Residual and reconstructed value at a boundary position for a chosen level.
/// L0 reconstructs exactly (residual 0, recon = truth); we reconstruct truth by
/// residual+recon consistency: for L0 the residual is zero and the reconstructed
/// value equals truth, which we recover as `l1_recon - l1_residual` (= truth).
fn residual_for_level(pos: &FacePos, level: Level) -> (Vec<Vec3>, Vec<Vec3>) {
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
    if report.composed_l1_aggregate.bricks == 0 && report.nonempty_bricks > 0 {
        log::info!(
            "[sh-analyze] NOTE: L1 is UNEVALUABLE on every non-empty brick — the grid is only \
             {} probe(s) tall, so each brick's 8 corner probes fall on the solid floor/ceiling \
             y-planes (all invalid). L1's corner basis collapses at this spacing; only L2 is \
             evaluable. L1 is reported as ineligible (never error-zero), so the sweep/seam use \
             L0/L2 only.",
            report.grid_dims[1]
        );
    }
    agg("base L1", &report.base_l1_aggregate);
    agg("base L2", &report.base_l2_aggregate);
    log::info!("[sh-analyze] === composed-receiver error (metric 2, PRIMARY) ===");
    agg("composed L1 (unweighted)", &report.composed_l1_aggregate);
    agg("composed L2 (unweighted)", &report.composed_l2_aggregate);
    agg(
        "composed L1 (cosine-weighted)",
        &report.composed_l1_weighted_aggregate,
    );
    agg(
        "composed L2 (cosine-weighted)",
        &report.composed_l2_weighted_aggregate,
    );
    if let Some(emitted) = &report.emitted_reconstruction {
        log::info!("[sh-analyze] === final emitted reconstruction (dense post-drop truth) ===");
        log::info!(
            "[sh-analyze] emitted: {} failing brick(s), truth map-p95 {:.6}, floor {:.6}, limits p95 {:.3} max {:.3}",
            emitted.failing_bricks,
            emitted.dense_truth_map_p95,
            emitted.darkness_floor,
            emitted.rel_p95_limit,
            emitted.rel_max_limit,
        );
    }

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
        "[sh-analyze] finalized delta entries {} — bit-zero fraction {:.4}, near-zero(<{:.0e}) fraction {:.4}",
        report.total_delta_entries,
        report.exact_zero_entry_fraction,
        report.near_zero_eps,
        report.near_zero_entry_fraction,
    );

    log::info!(
        "[sh-analyze] === threshold sweep (composed-error gate; L0/L1/L2 histogram + projected savings) ==="
    );
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
    fn tile_magnitude_uses_max_per_channel_and_skips_invalid() {
        let texels = 4;
        // A constant field: magnitude equals the per-channel value everywhere.
        let stats = tile_magnitude(&const_tiles(2.0, texels), texels);
        assert_eq!(stats.max, 2.0);
        assert_eq!(stats.mean, 2.0);
        assert_eq!(stats.p95, 2.0);
        assert_eq!(stats.texel_samples, (PROBES_PER_CELL * texels) as u64);

        // Anisotropic channels: magnitude is the max-per-channel reduction, and
        // the same reduction `texel_error` applies to differences, so the two
        // are directly comparable as a relative deviation.
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        tiles[0] = Some(vec![Vec3::new(0.1, 3.0, -0.2); texels]);
        let stats = tile_magnitude(&tiles, texels);
        assert_eq!(stats.max, 3.0);
        assert_eq!(stats.texel_samples, texels as u64);

        // No valid probes → zeroed record, never a spurious magnitude.
        let empty: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        let stats = tile_magnitude(&empty, texels);
        assert_eq!(stats.texel_samples, 0);
        assert_eq!(stats.max, 0.0);
    }

    #[test]
    fn emitted_error_uses_dense_truth_not_emitted_magnitude() {
        let texels = 1;
        let mut truth: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        let mut emitted: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        truth[0] = Some(vec![Vec3::splat(10.0)]);
        emitted[0] = Some(vec![Vec3::ZERO]);

        let magnitude = tile_magnitude(&truth, texels);
        let error = emitted_error(&truth, &emitted, texels);
        assert_eq!(magnitude.p95, 10.0, "denominator is dense truth");
        assert_eq!(error.p95, 10.0, "error is emitted minus dense truth");
        assert_eq!(error.max, 10.0);
    }

    #[test]
    fn emitted_floor_uses_classifier_truncated_map_p95_not_rounded_p95() {
        // For three records, floor(0.95 * (n - 1)) = 1; the generic per-brick
        // percentile helper rounds and would select index 2 instead.
        let (map_p95, floor) = classifier_darkness_floor(&[1.0, 10.0, 100.0], 0.02);
        assert_eq!(map_p95, 10.0);
        assert!((floor - 0.2).abs() < 1.0e-6);
    }

    fn all_valid_payload(entries: usize, probe_f16_stride: usize) -> Vec<u16> {
        let mut payload = vec![0; entries * PROBES_PER_CELL * probe_f16_stride];
        for entry in 0..entries {
            for local in 0..PROBES_PER_CELL {
                payload[(entry * PROBES_PER_CELL + local) * probe_f16_stride] =
                    (entry * PROBES_PER_CELL + local) as u16 + 1;
            }
        }
        payload
    }

    fn assert_all_valid_payload_uses_dense_offsets(view: &DeltaView<'_>) {
        let probe_f16_stride = view.probe_f16_stride();
        for entry in 0..2 {
            for local in 0..PROBES_PER_CELL {
                let offset = view
                    .resolve_probe_f16_offset(0, entry, local)
                    .expect("all-valid entry must resolve every local probe");
                assert_eq!(offset, (entry * PROBES_PER_CELL + local) * probe_f16_stride);
                assert_eq!(
                    view.subblocks[offset],
                    (entry * PROBES_PER_CELL + local) as u16 + 1,
                );
            }
        }
    }

    #[test]
    fn delta_views_preserve_all_valid_dense_payload_bytes_for_all_section_ids() {
        let probe_f16_stride = 6 * 6 * 4;
        let payload = all_valid_payload(2, probe_f16_stride);
        let indirect = DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 2],
            affinity_lights: vec![0, 0],
            delta_subblocks: payload.clone(),
        };
        let direct = DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 2],
            affinity_lights: vec![0, 0],
            delta_subblocks: payload.clone(),
        };
        let animated_direct = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 2],
            affinity_lights: vec![0, 0],
            delta_subblocks: payload,
        };

        assert_all_valid_payload_uses_dense_offsets(&DeltaView::from_indirect(&indirect));
        assert_all_valid_payload_uses_dense_offsets(&DeltaView::from_direct(&direct));
        assert_all_valid_payload_uses_dense_offsets(&DeltaView::from_anim_direct(&animated_direct));
    }

    #[test]
    fn delta_view_resolves_mixed_masks_by_within_cell_rank() {
        let mask = (1u64 << 0) | (1u64 << 2) | (1u64 << 5);
        let masks = [mask];
        let offsets = [0, 2];
        let payload = vec![0; 2 * 3 * 4];
        let view = DeltaView::new([1, 1, 1], 1, &masks, &offsets, &payload);

        assert_eq!(view.resolve_probe_f16_offset(0, 0, 0), Some(0));
        assert_eq!(view.resolve_probe_f16_offset(0, 0, 2), Some(4));
        assert_eq!(view.resolve_probe_f16_offset(0, 0, 5), Some(8));
        assert_eq!(view.resolve_probe_f16_offset(0, 1, 0), Some(12));
        assert_eq!(view.resolve_probe_f16_offset(0, 1, 2), Some(16));
        assert_eq!(view.resolve_probe_f16_offset(0, 1, 5), Some(20));
        assert_eq!(view.resolve_probe_f16_offset(0, 0, 1), None);
        assert_eq!(view.resolve_probe_f16_offset(0, 1, 63), None);
    }

    #[test]
    fn delta_view_keeps_all_invalid_entries_at_zero_length() {
        let masks = [0];
        let offsets = [0, 1];
        let payload = [];
        let view = DeltaView::new([1, 1, 1], 1, &masks, &offsets, &payload);

        assert_eq!(view.entry_payload_range(0), Some(0..0));
        assert_eq!(view.entry_payload_f16_count(0), 0);
        assert_eq!(entry_zeroness(&view, 0), (true, true));
        assert_eq!(view.resolve_probe_f16_offset(0, 0, 0), None);
        assert_eq!(view.resolve_probe_f16_offset(0, 0, 63), None);
    }

    #[test]
    fn analysis_keeps_valid_cell_zero_direct_fallback_in_l1_l2_projections() {
        // Regression: a selected all-zero id 41 fallback was counted as dropped
        // even though the finalized CSR retains it to preserve light coverage.
        let mut base = OctahedralShVolumeSection::placeholder();
        base.grid_dimensions = [1, 1, 1];
        base.tile_dimension = 1;
        base.tile_border = 0;
        base.atlas_dimensions = [1, 1];
        base.layer_count = 1;
        base.tiles_per_layer = 1;
        base.atlas_tiles_per_row = 1;
        base.compact_atlas_dimensions = [1, 1];
        base.compact_atlas_tiles_per_row = 1;
        base.compact_atlas_tiles_per_layer = 1;
        base.compact_atlas_layer_count = 1;
        base.irradiance_format = postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
        base.compact_atlas = vec![0; 8];

        let direct = DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 1,
            tile_border: 0,
            valid_probe_masks: vec![1],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: vec![0; 4],
        };
        let validity = [1];
        let thresholds = [0.0];
        let report = run_analysis(&AnalyzeInputs {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dims: [1, 1, 1],
            validity: &validity,
            base_indirect: &base,
            base_direct: None,
            delta_indirect: None,
            delta_direct: Some(&direct),
            delta_anim_direct: None,
            protect_aabbs: &[],
            thresholds: &thresholds,
        });

        let direct_bytes = report
            .section_bytes
            .iter()
            .find(|section| section.id == 41)
            .expect("id 41 accounting must be present");
        assert_eq!(direct_bytes.compacted_bytes, 8);
        assert_eq!(direct_bytes.exact_zero_dropped_bytes, 0);
        assert_eq!(direct_bytes.coarsen_all_l1_bytes, 8);
        assert_eq!(direct_bytes.coarsen_all_l2_bytes, 8);
        assert_eq!(report.exact_zero_entry_fraction, 1.0);
        assert_eq!(report.sweep[0].projected_bytes, 24);
    }

    #[test]
    fn delta_view_prefixes_each_entry_after_variable_length_cells() {
        let masks = [
            (1u64 << 0) | (1u64 << 3),
            0,
            (1u64 << 1) | (1u64 << 4) | (1u64 << 7),
        ];
        let offsets = [0, 2, 3, 4];
        let payload = vec![0; 28];
        let view = DeltaView::new([3, 1, 1], 1, &masks, &offsets, &payload);

        assert_eq!(view.entry_payload_offsets, vec![0, 8, 16, 16, 28]);
        assert_eq!(view.resolve_probe_f16_offset(0, 1, 3), Some(12));
        assert_eq!(view.entry_payload_range(2), Some(16..16));
        assert_eq!(view.resolve_probe_f16_offset(1, 2, 0), None);
        assert_eq!(view.resolve_probe_f16_offset(2, 3, 7), Some(24));
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
        assert!(
            err.max < 1e-4,
            "linear ramp L1 error must be ~0, got {}",
            err.max
        );
    }

    // The pure trilinear-weight / corner-index tests moved with the math into
    // `postretro_level_format::sh_reconstruct` (task G1).

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
        assert!(
            max_seam > 0.1,
            "L0|L2 seam on a gradient must be non-zero, got {max_seam}"
        );
    }
}
