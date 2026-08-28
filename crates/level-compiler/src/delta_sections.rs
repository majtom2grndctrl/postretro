//! Carries all baked delta sections through the compiler's post-bake seam.
//! See: context/lib/build_pipeline.md §PRL section IDs.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DELTA_TILE_TEXEL_F16_COUNT, DeltaShVolumesSection, PROBES_PER_CELL,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::sh_reconstruct::{
    Level, Tile, kept_mask, reconstruct_l1_tile, reconstruct_l2_tile, zero_tile,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::delta_drop_policy::{
    DropStats, ScriptMutableDescriptorSlots, drop_animated_direct_zero_entries,
    drop_direct_zero_entries, drop_indirect_zero_entries,
};

/// Default aggregate raw payload cap for ids 27, 41, and 45 on desktop maps.
pub(crate) const DEFAULT_MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// Authoring budget for the aggregate raw payload. Crossing this target warns
/// but never changes emitted detail or substitutes for the production cap.
const DIAGNOSTIC_MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// Compiler-only configuration for the post-bake delta-section policy.
///
/// The cap is carried with the baked sections so the policy can enforce it
/// before packing without leaking an implementation choice into the PRL wire
/// format or runtime representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeltaSectionConfig {
    pub max_payload_bytes: u64,
}

impl Default for DeltaSectionConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
        }
    }
}

/// Delta sections after all three delta bakes have completed.
///
/// This is intentionally an owned handoff. The next compiler-only policy can
/// inspect or replace these sections before packing while the pipeline and
/// runtime consume compact variable-stride contracts for ids 27, 41, and 45.
pub(crate) struct PostBakeDeltaSections {
    pub config: DeltaSectionConfig,
    pub indirect: Option<DeltaShVolumesSection>,
    pub entity_shadow_lights: Option<EntityShadowLightsSection>,
    pub direct: Option<DirectShDeltaVolumesSection>,
    pub animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
}

impl PostBakeDeltaSections {
    pub(crate) fn new(
        config: DeltaSectionConfig,
        indirect: Option<DeltaShVolumesSection>,
        entity_shadow_lights: Option<EntityShadowLightsSection>,
        direct: Option<DirectShDeltaVolumesSection>,
        animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
    ) -> Self {
        Self {
            config,
            indirect,
            entity_shadow_lights,
            direct,
            animated_direct,
        }
    }

    /// Apply the compiler-only exact-zero policy while all delta sections are
    /// still owned by the pipeline. The wire spelling of a dropped record is
    /// the existing CSR absence spelling for a zero contribution. This pass
    /// deliberately runs before valid-probe compaction, while every retained
    /// entry still has its dense 64-probe bake payload.
    pub(crate) fn apply_exact_zero_drop_policy(
        &mut self,
        mutable_descriptors: &ScriptMutableDescriptorSlots,
    ) -> anyhow::Result<()> {
        if let Some(section) = self.indirect.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let (section, stats) = drop_indirect_zero_entries(&section, mutable_descriptors);
            log_drop_summary(
                "DeltaShVolumes",
                stats,
                input_payload_bytes,
                payload_bytes(&section.delta_subblocks),
                csr_bytes(&section.affinity_offsets, &section.affinity_lights),
                false,
            );
            // id 27's existing base-only spelling is an emitted empty section.
            self.indirect = Some(section);
        }

        if let Some(section) = self.direct.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let (section, stats) = drop_direct_zero_entries(&section);
            log_drop_summary(
                "DirectShDeltaVolumes",
                stats,
                input_payload_bytes,
                payload_bytes(&section.delta_subblocks),
                csr_bytes(&section.affinity_offsets, &section.affinity_lights),
                false,
            );
            self.direct = Some(section);
        }

        if let Some(section) = self.animated_direct.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let (section, stats) = drop_animated_direct_zero_entries(&section, mutable_descriptors);
            let is_empty = section.affinity_lights.is_empty();
            log_drop_summary(
                "AnimatedDirectShDeltaVolumes",
                stats,
                input_payload_bytes,
                payload_bytes(&section.delta_subblocks),
                csr_bytes(&section.affinity_offsets, &section.affinity_lights),
                is_empty,
            );
            // id 45 has an optional wire contract: an empty post-drop section
            // is absent, rather than a header with no CSR records.
            self.animated_direct = (!is_empty).then_some(section);
        }

        Ok(())
    }

    /// Keep runtime-unbounded indirect and animated-direct contributions on
    /// the dense representation. Only id 41 currently has a runtime-safe
    /// weight envelope, so ids 27 and 45 must remain byte-compatible with the
    /// uniform-L0 path when coarsening is requested.
    pub(crate) fn enforce_id41_only_coarsening_policy(&mut self) {
        if let Some(section) = self.indirect.as_mut() {
            section.cell_levels.fill(Level::L0.to_u8());
        }
        if let Some(section) = self.animated_direct.as_mut() {
            section.cell_levels.fill(Level::L0.to_u8());
        }
    }

    /// Losslessly compact delta payloads to the id-34-valid probes in each
    /// affinity cell. The base volume is the sole validity authority; each
    /// section's own masks are replaced rather than consulted.
    ///
    /// The delta bakes remain dense so exact-zero dropping above observes their
    /// original 64-probe records. This pass then rewrites each post-drop CSR
    /// entry in canonical cell order, retaining valid local tiles only in
    /// x-fastest order.
    pub(crate) fn apply_valid_probe_compaction(
        &mut self,
        base: &OctahedralShVolumeSection,
    ) -> anyhow::Result<()> {
        if let Some(section) = self.indirect.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let compacted = compact_indirect_valid_probes(&section, base)?;
            let compacted_payload_bytes = payload_bytes(&compacted.delta_subblocks);
            let valid_probe_count: u64 = compacted
                .valid_probe_masks
                .iter()
                .map(|mask| u64::from(mask.count_ones()))
                .sum();
            log::info!(
                "[Compiler] DeltaShVolumes valid-probe compaction: {} CSR entr(y/ies), \
                 {valid_probe_count} valid affinity-local probe(s), {input_payload_bytes} -> \
                 {compacted_payload_bytes} raw payload bytes",
                compacted.affinity_lights.len(),
            );
            self.indirect = Some(compacted);
        }

        if let Some(section) = self.direct.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let compacted = compact_direct_valid_probes(&section, base)?;
            let compacted_payload_bytes = payload_bytes(&compacted.delta_subblocks);
            let valid_probe_count: u64 = compacted
                .valid_probe_masks
                .iter()
                .map(|mask| u64::from(mask.count_ones()))
                .sum();
            log::info!(
                "[Compiler] DirectShDeltaVolumes valid-probe compaction: {} CSR entr(y/ies), \
                 {valid_probe_count} valid affinity-local probe(s), {input_payload_bytes} -> \
                 {compacted_payload_bytes} raw payload bytes",
                compacted.affinity_lights.len(),
            );
            self.direct = Some(compacted);
        }

        if let Some(section) = self.animated_direct.take() {
            let input_payload_bytes = payload_bytes(&section.delta_subblocks);
            let compacted = compact_animated_direct_valid_probes(&section, base)?;
            let compacted_payload_bytes = payload_bytes(&compacted.delta_subblocks);
            let valid_probe_count: u64 = compacted
                .valid_probe_masks
                .iter()
                .map(|mask| u64::from(mask.count_ones()))
                .sum();
            log::info!(
                "[Compiler] AnimatedDirectShDeltaVolumes valid-probe compaction: {} CSR entr(y/ies), \
                 {valid_probe_count} valid affinity-local probe(s), {input_payload_bytes} -> \
                 {compacted_payload_bytes} raw payload bytes",
                compacted.affinity_lights.len(),
            );
            // Exact-zero drop owns id 45's optional-section normalization.
            // Compaction preserves post-drop CSR entries, including a retained
            // script-mutable zero-length entry in an all-invalid cell.
            self.animated_direct = Some(compacted);
        }
        Ok(())
    }

    /// Enforce the production cap on raw emitted delta blocks only, warning
    /// when the post-compaction payload crosses the lower authoring target.
    /// Ids 27, 41, and 45 have completed valid-probe compaction before this
    /// call. Header, descriptor, and CSR bytes remain intentionally out of
    /// budget.
    pub(crate) fn enforce_payload_cap(&self) -> anyhow::Result<()> {
        let payload = DeltaPayloadBytes {
            indirect: self
                .indirect
                .as_ref()
                .map_or(0, |section| payload_bytes(&section.delta_subblocks)),
            direct: self
                .direct
                .as_ref()
                .map_or(0, |section| payload_bytes(&section.delta_subblocks)),
            animated_direct: self
                .animated_direct
                .as_ref()
                .map_or(0, |section| payload_bytes(&section.delta_subblocks)),
        };
        let total = payload
            .indirect
            .checked_add(payload.direct)
            .and_then(|bytes| bytes.checked_add(payload.animated_direct))
            .ok_or_else(|| anyhow::anyhow!("SH delta payload byte total overflow"))?;
        let overage = total.saturating_sub(self.config.max_payload_bytes);
        log::info!(
            "[Compiler] SH delta payload cap: id 27 {} bytes, id 41 {} bytes, \\
             id 45 {} bytes, total {total} bytes, cap {} bytes, overage {overage} bytes",
            payload.indirect,
            payload.direct,
            payload.animated_direct,
            self.config.max_payload_bytes,
        );
        anyhow::ensure!(
            total <= self.config.max_payload_bytes,
            "SH delta payload cap exceeded before packing: id 27 {} bytes, id 41 {} bytes, \\
             id 45 {} bytes; total {total} bytes exceeds cap {} bytes by {overage} bytes",
            payload.indirect,
            payload.direct,
            payload.animated_direct,
            self.config.max_payload_bytes,
        );
        warn_if_over_authoring_budget(payload, total, self.config.max_payload_bytes);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct DeltaPayloadBytes {
    indirect: u64,
    direct: u64,
    animated_direct: u64,
}

fn warn_if_over_authoring_budget(payload: DeltaPayloadBytes, total: u64, cap: u64) {
    if total > DIAGNOSTIC_MAX_PAYLOAD_BYTES {
        log::warn!(
            "[Compiler] SH delta authoring budget warning: id 27 {} bytes, id 41 {} bytes, \\
             id 45 {} bytes; total {total} bytes exceeds the 64 MiB ({} byte) diagnostic target but remains within the {} byte production cap",
            payload.indirect,
            payload.direct,
            payload.animated_direct,
            DIAGNOSTIC_MAX_PAYLOAD_BYTES,
            cap,
        );
    }
}

fn compact_indirect_valid_probes(
    section: &DeltaShVolumesSection,
    base: &OctahedralShVolumeSection,
) -> anyhow::Result<DeltaShVolumesSection> {
    let compacted = compact_dense_valid_probe_payload(
        DenseDeltaPayload {
            section_name: "DeltaShVolumes",
            affinity_factor: section.affinity_factor,
            affinity_dims: section.affinity_dims,
            affinity_offsets: &section.affinity_offsets,
            affinity_lights: &section.affinity_lights,
            cell_levels: &section.cell_levels,
            delta_subblocks: &section.delta_subblocks,
            probe_stride: section.delta_probe_f16_stride(),
        },
        base,
    )?;

    Ok(DeltaShVolumesSection {
        affinity_factor: section.affinity_factor,
        affinity_dims: section.affinity_dims,
        tile_dimension: section.tile_dimension,
        tile_border: section.tile_border,
        animation_descriptor_indices: section.animation_descriptor_indices.clone(),
        valid_probe_masks: compacted.valid_probe_masks,
        cell_levels: section.cell_levels.clone(),
        affinity_offsets: section.affinity_offsets.clone(),
        affinity_lights: section.affinity_lights.clone(),
        delta_subblocks: compacted.delta_subblocks,
    })
}

pub(crate) fn compact_direct_valid_probes(
    section: &DirectShDeltaVolumesSection,
    base: &OctahedralShVolumeSection,
) -> anyhow::Result<DirectShDeltaVolumesSection> {
    let compacted = compact_dense_valid_probe_payload(
        DenseDeltaPayload {
            section_name: "DirectShDeltaVolumes",
            affinity_factor: section.affinity_factor,
            affinity_dims: section.affinity_dims,
            affinity_offsets: &section.affinity_offsets,
            affinity_lights: &section.affinity_lights,
            cell_levels: &section.cell_levels,
            delta_subblocks: &section.delta_subblocks,
            probe_stride: section.delta_probe_f16_stride(),
        },
        base,
    )?;

    Ok(DirectShDeltaVolumesSection {
        affinity_factor: section.affinity_factor,
        affinity_dims: section.affinity_dims,
        tile_dimension: section.tile_dimension,
        tile_border: section.tile_border,
        valid_probe_masks: compacted.valid_probe_masks,
        cell_levels: section.cell_levels.clone(),
        affinity_offsets: section.affinity_offsets.clone(),
        affinity_lights: section.affinity_lights.clone(),
        delta_subblocks: compacted.delta_subblocks,
    })
}

fn compact_animated_direct_valid_probes(
    section: &AnimatedDirectShDeltaVolumesSection,
    base: &OctahedralShVolumeSection,
) -> anyhow::Result<AnimatedDirectShDeltaVolumesSection> {
    let compacted = compact_dense_valid_probe_payload(
        DenseDeltaPayload {
            section_name: "AnimatedDirectShDeltaVolumes",
            affinity_factor: section.affinity_factor,
            affinity_dims: section.affinity_dims,
            affinity_offsets: &section.affinity_offsets,
            affinity_lights: &section.affinity_lights,
            cell_levels: &section.cell_levels,
            delta_subblocks: &section.delta_subblocks,
            probe_stride: section.delta_probe_f16_stride(),
        },
        base,
    )?;

    Ok(AnimatedDirectShDeltaVolumesSection {
        affinity_factor: section.affinity_factor,
        affinity_dims: section.affinity_dims,
        tile_dimension: section.tile_dimension,
        tile_border: section.tile_border,
        animation_descriptor_indices: section.animation_descriptor_indices.clone(),
        valid_probe_masks: compacted.valid_probe_masks,
        cell_levels: section.cell_levels.clone(),
        affinity_offsets: section.affinity_offsets.clone(),
        affinity_lights: section.affinity_lights.clone(),
        delta_subblocks: compacted.delta_subblocks,
    })
}

struct CompactedDeltaPayload {
    valid_probe_masks: Vec<u64>,
    delta_subblocks: Vec<u16>,
}

struct DenseDeltaPayload<'a> {
    section_name: &'static str,
    affinity_factor: u8,
    affinity_dims: [u32; 3],
    affinity_offsets: &'a [u32],
    affinity_lights: &'a [u32],
    cell_levels: &'a [u8],
    delta_subblocks: &'a [u16],
    probe_stride: usize,
}

fn compact_dense_valid_probe_payload(
    payload: DenseDeltaPayload<'_>,
    base: &OctahedralShVolumeSection,
) -> anyhow::Result<CompactedDeltaPayload> {
    let DenseDeltaPayload {
        section_name,
        affinity_factor,
        affinity_dims,
        affinity_offsets,
        affinity_lights,
        cell_levels,
        delta_subblocks,
        probe_stride,
    } = payload;
    let affinity_cell_count =
        affinity_dims[0] as usize * affinity_dims[1] as usize * affinity_dims[2] as usize;
    anyhow::ensure!(
        affinity_factor == AFFINITY_FACTOR,
        "{section_name} affinity factor {affinity_factor} does not match the valid-probe compaction factor {AFFINITY_FACTOR}",
    );
    anyhow::ensure!(
        affinity_offsets.len() == affinity_cell_count.saturating_add(1)
            && affinity_offsets.first().copied() == Some(0)
            && affinity_offsets.windows(2).all(|pair| pair[0] <= pair[1])
            && affinity_offsets.last().copied() == u32::try_from(affinity_lights.len()).ok(),
        "{section_name} has an invalid CSR shape before valid-probe compaction"
    );
    anyhow::ensure!(
        affinity_dims == affinity_dims_for_grid(base.grid_dimensions, affinity_factor),
        "{section_name} affinity dims {affinity_dims:?} do not match OctahedralShVolume grid {:?} for valid-probe compaction",
        base.grid_dimensions,
    );
    anyhow::ensure!(
        base.probes.len() == base.total_probes(),
        "OctahedralShVolume probe metadata length {} does not match its grid volume {} during {section_name} compaction",
        base.probes.len(),
        base.total_probes(),
    );

    let dense_entry_stride = PROBES_PER_CELL
        .checked_mul(probe_stride)
        .ok_or_else(|| anyhow::anyhow!("{section_name} dense entry stride overflow"))?;
    let expected_dense_payload = affinity_lights
        .len()
        .checked_mul(dense_entry_stride)
        .ok_or_else(|| anyhow::anyhow!("{section_name} dense payload length overflow"))?;
    anyhow::ensure!(
        delta_subblocks.len() == expected_dense_payload,
        "{section_name} must retain dense 64-probe payloads until valid-probe compaction"
    );

    let valid_probe_masks: Vec<u64> = (0..affinity_cell_count)
        .map(|cell| valid_probe_mask_for_affinity_cell(base, affinity_dims, cell))
        .collect();
    // Each cell's stored (kept) probe set is derived from its coarsening level
    // and full validity. At L0 `kept_mask == validity`, so this is byte-for-byte
    // identical to the pre-coarsening path; L1/L2 store a coarser lattice while
    // the emitted `valid_probe_masks` remains full validity.
    let levels: Vec<Level> = (0..affinity_cell_count)
        .map(|cell| {
            let byte = *cell_levels.get(cell).ok_or_else(|| {
                anyhow::anyhow!(
                    "{section_name} cell_levels length {} is shorter than affinity cell count {affinity_cell_count} during valid-probe compaction",
                    cell_levels.len(),
                )
            })?;
            Level::from_u8(byte).ok_or_else(|| {
                anyhow::anyhow!(
                    "{section_name} cell level byte {byte} is out of range during valid-probe compaction"
                )
            })
        })
        .collect::<anyhow::Result<Vec<Level>>>()?;
    let kept_masks: Vec<u64> = levels
        .iter()
        .zip(&valid_probe_masks)
        .map(|(&level, &validity)| kept_mask(level, validity))
        .collect();
    let compact_tile_count = (0..affinity_cell_count).try_fold(0usize, |total, cell| {
        let entry_count = (affinity_offsets[cell + 1] - affinity_offsets[cell]) as usize;
        let cell_tiles = entry_count
            .checked_mul(kept_masks[cell].count_ones() as usize)
            .ok_or_else(|| anyhow::anyhow!("{section_name} compact tile count overflow"))?;
        total
            .checked_add(cell_tiles)
            .ok_or_else(|| anyhow::anyhow!("{section_name} compact tile count overflow"))
    })?;
    let capacity = compact_tile_count
        .checked_mul(probe_stride)
        .ok_or_else(|| anyhow::anyhow!("{section_name} compact payload length overflow"))?;
    let mut compacted_subblocks = Vec::with_capacity(capacity);

    for cell in 0..affinity_cell_count {
        let validity = valid_probe_masks[cell];
        let level = levels[cell];
        let kept = kept_masks[cell];
        let start = affinity_offsets[cell] as usize;
        let end = affinity_offsets[cell + 1] as usize;
        for entry in start..end {
            let dense_entry =
                &delta_subblocks[entry * dense_entry_stride..(entry + 1) * dense_entry_stride];
            match level {
                // L0/L1 copy each kept probe's raw dense tile in x-fastest order.
                // The copy loop is identical to the pre-coarsening path except
                // that it is gated by `kept` (which equals validity at L0).
                Level::L0 | Level::L1 => {
                    for local_probe in 0..PROBES_PER_CELL {
                        if kept & (1u64 << local_probe) == 0 {
                            continue;
                        }
                        let tile_start = local_probe * probe_stride;
                        compacted_subblocks
                            .extend_from_slice(&dense_entry[tile_start..tile_start + probe_stride]);
                    }
                }
                // L2 stores exactly one synthesized brick-mean tile at the
                // representative kept slot, over this entry's own valid probes.
                Level::L2 => {
                    if kept == 0 {
                        continue;
                    }
                    let mean_tile = synthesize_l2_mean_tile(dense_entry, validity, probe_stride);
                    compacted_subblocks.extend_from_slice(&mean_tile);
                }
            }
        }
    }

    Ok(CompactedDeltaPayload {
        valid_probe_masks,
        delta_subblocks: compacted_subblocks,
    })
}

/// Synthesize the L2 brick-mean tile for one dense delta entry, encoded as an
/// f16 RGBA tile. The mean is taken over the entry's own VALID probe tiles using
/// [`reconstruct_l2_tile`] — the exact definition the classifier measured L2
/// error against and the render-cpu golden reads back — not a per-channel average
/// rolled here. The caller guarantees `validity != 0`, so the mean is defined.
fn synthesize_l2_mean_tile(dense_entry: &[u16], validity: u64, probe_stride: usize) -> Vec<u16> {
    let tile_texels = probe_stride / DELTA_TILE_TEXEL_F16_COUNT;
    let mut valid_tiles: [Option<Vec<glam::Vec3>>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
    let mut remaining = validity;
    while remaining != 0 {
        let local = remaining.trailing_zeros() as usize;
        remaining &= remaining - 1;
        let base = local * probe_stride;
        let tile: Vec<glam::Vec3> = (0..tile_texels)
            .map(|texel| {
                let i = base + texel * DELTA_TILE_TEXEL_F16_COUNT;
                glam::Vec3::new(
                    crate::sh_bake::f16_bits_to_f32(dense_entry[i]),
                    crate::sh_bake::f16_bits_to_f32(dense_entry[i + 1]),
                    crate::sh_bake::f16_bits_to_f32(dense_entry[i + 2]),
                )
            })
            .collect();
        valid_tiles[local] = Some(tile);
    }
    let mean = reconstruct_l2_tile(&valid_tiles, tile_texels)
        .expect("a non-empty valid probe set yields an L2 brick mean");
    let valid_alpha = f32_to_f16_bits(1.0);
    let mut encoded = Vec::with_capacity(probe_stride);
    for texel in mean {
        encoded.push(f32_to_f16_bits(texel.x));
        encoded.push(f32_to_f16_bits(texel.y));
        encoded.push(f32_to_f16_bits(texel.z));
        encoded.push(valid_alpha);
    }
    encoded
}

/// Borrowed, validated view of one finalized compact delta section.
///
/// The view precomputes each CSR entry's variable-stride f16 offset once. It is
/// shared by compiler diagnostics that need the exact tile represented by final
/// `cell_levels` without reimplementing compact offset/rank arithmetic.
pub(crate) struct EmittedDeltaSectionRef<'a> {
    affinity_offsets: &'a [u32],
    valid_probe_masks: &'a [u64],
    cell_levels: &'a [u8],
    delta_subblocks: &'a [u16],
    entry_f16_offsets: Vec<usize>,
    tile_dimension: usize,
    tile_border: usize,
    probe_stride: usize,
    interior_texels: usize,
}

impl<'a> EmittedDeltaSectionRef<'a> {
    pub(crate) fn from_indirect(section: &'a DeltaShVolumesSection) -> anyhow::Result<Self> {
        Self::new(
            &section.affinity_offsets,
            &section.valid_probe_masks,
            &section.cell_levels,
            &section.delta_subblocks,
            section.tile_dimension,
            section.tile_border,
        )
    }

    pub(crate) fn from_direct(section: &'a DirectShDeltaVolumesSection) -> anyhow::Result<Self> {
        Self::new(
            &section.affinity_offsets,
            &section.valid_probe_masks,
            &section.cell_levels,
            &section.delta_subblocks,
            section.tile_dimension,
            section.tile_border,
        )
    }

    pub(crate) fn from_animated_direct(
        section: &'a AnimatedDirectShDeltaVolumesSection,
    ) -> anyhow::Result<Self> {
        Self::new(
            &section.affinity_offsets,
            &section.valid_probe_masks,
            &section.cell_levels,
            &section.delta_subblocks,
            section.tile_dimension,
            section.tile_border,
        )
    }

    fn new(
        affinity_offsets: &'a [u32],
        valid_probe_masks: &'a [u64],
        cell_levels: &'a [u8],
        delta_subblocks: &'a [u16],
        tile_dimension: u32,
        tile_border: u32,
    ) -> anyhow::Result<Self> {
        let cell_count = valid_probe_masks.len();
        anyhow::ensure!(
            cell_levels.len() == cell_count,
            "emitted delta view: {} levels for {cell_count} cells",
            cell_levels.len()
        );
        anyhow::ensure!(
            affinity_offsets.len() == cell_count + 1,
            "emitted delta view: {} CSR offsets for {cell_count} cells",
            affinity_offsets.len()
        );
        anyhow::ensure!(
            affinity_offsets.first().copied() == Some(0),
            "emitted delta view: first CSR offset must be zero"
        );
        anyhow::ensure!(
            affinity_offsets.windows(2).all(|pair| pair[0] <= pair[1]),
            "emitted delta view: CSR offsets must be monotonic"
        );
        let tile_dimension = tile_dimension as usize;
        let tile_border = tile_border as usize;
        anyhow::ensure!(
            tile_dimension > tile_border.saturating_mul(2),
            "emitted delta view: tile dimension {tile_dimension} must exceed twice border {tile_border}"
        );
        let probe_stride = tile_dimension
            .checked_mul(tile_dimension)
            .and_then(|texels| texels.checked_mul(DELTA_TILE_TEXEL_F16_COUNT))
            .ok_or_else(|| anyhow::anyhow!("emitted delta view: probe stride overflow"))?;
        let interior_edge = tile_dimension - tile_border * 2;
        let interior_texels = interior_edge
            .checked_mul(interior_edge)
            .ok_or_else(|| anyhow::anyhow!("emitted delta view: interior area overflow"))?;
        let entry_count = affinity_offsets.last().copied().unwrap_or(0) as usize;
        let mut entry_f16_offsets = Vec::with_capacity(entry_count);
        let mut cursor = 0usize;
        for cell in 0..cell_count {
            let level = Level::from_u8(cell_levels[cell]).ok_or_else(|| {
                anyhow::anyhow!(
                    "emitted delta view: cell {cell} has invalid level {}",
                    cell_levels[cell]
                )
            })?;
            let stored_tiles = kept_mask(level, valid_probe_masks[cell]).count_ones() as usize;
            let entry_stride = stored_tiles
                .checked_mul(probe_stride)
                .ok_or_else(|| anyhow::anyhow!("emitted delta view: entry stride overflow"))?;
            for _ in affinity_offsets[cell]..affinity_offsets[cell + 1] {
                entry_f16_offsets.push(cursor);
                cursor = cursor.checked_add(entry_stride).ok_or_else(|| {
                    anyhow::anyhow!("emitted delta view: payload offset overflow")
                })?;
            }
        }
        anyhow::ensure!(
            entry_f16_offsets.len() == entry_count,
            "emitted delta view: walked {} entries, CSR declares {entry_count}",
            entry_f16_offsets.len()
        );
        anyhow::ensure!(
            cursor == delta_subblocks.len(),
            "emitted delta view: represented length is {cursor} f16 values, payload has {}",
            delta_subblocks.len()
        );
        Ok(Self {
            affinity_offsets,
            valid_probe_masks,
            cell_levels,
            delta_subblocks,
            entry_f16_offsets,
            tile_dimension,
            tile_border,
            probe_stride,
            interior_texels,
        })
    }

    pub(crate) fn cell_count(&self) -> usize {
        self.valid_probe_masks.len()
    }

    pub(crate) fn entry_range(&self, cell: usize) -> Option<std::ops::Range<usize>> {
        Some(
            *self.affinity_offsets.get(cell)? as usize
                ..*self.affinity_offsets.get(cell + 1)? as usize,
        )
    }

    pub(crate) fn valid_probe_mask(&self, cell: usize) -> Option<u64> {
        self.valid_probe_masks.get(cell).copied()
    }

    pub(crate) fn level(&self, cell: usize) -> Option<Level> {
        Level::from_u8(*self.cell_levels.get(cell)?)
    }

    pub(crate) fn interior_texels(&self) -> usize {
        self.interior_texels
    }

    fn decode_interior(&self, tile_start: usize) -> anyhow::Result<Tile> {
        let tile_end = tile_start
            .checked_add(self.probe_stride)
            .ok_or_else(|| anyhow::anyhow!("emitted delta view: tile offset overflow"))?;
        anyhow::ensure!(
            tile_end <= self.delta_subblocks.len(),
            "emitted delta view: tile [{tile_start}..{tile_end}) exceeds payload length {}",
            self.delta_subblocks.len()
        );
        let mut tile = Vec::with_capacity(self.interior_texels);
        for y in self.tile_border..self.tile_dimension - self.tile_border {
            for x in self.tile_border..self.tile_dimension - self.tile_border {
                let i = tile_start + (y * self.tile_dimension + x) * DELTA_TILE_TEXEL_F16_COUNT;
                tile.push(glam::Vec3::new(
                    crate::sh_bake::f16_bits_to_f32(self.delta_subblocks[i]),
                    crate::sh_bake::f16_bits_to_f32(self.delta_subblocks[i + 1]),
                    crate::sh_bake::f16_bits_to_f32(self.delta_subblocks[i + 2]),
                ));
            }
        }
        Ok(tile)
    }

    /// Reconstruct one entry/local tile exactly as the compose shader represents
    /// it. `None` means the target probe is invalid. Sparse-L1 targets with no
    /// positively weighted kept corner return an explicit zero tile.
    pub(crate) fn reconstruct_entry_tile(
        &self,
        cell: usize,
        entry: usize,
        local_probe: usize,
    ) -> anyhow::Result<Option<Tile>> {
        anyhow::ensure!(
            local_probe < PROBES_PER_CELL,
            "emitted delta view: local probe {local_probe} is outside 0..{PROBES_PER_CELL}"
        );
        let range = self
            .entry_range(cell)
            .ok_or_else(|| anyhow::anyhow!("emitted delta view: cell {cell} is out of range"))?;
        anyhow::ensure!(
            range.contains(&entry),
            "emitted delta view: entry {entry} does not belong to cell {cell}"
        );
        let validity = self.valid_probe_masks[cell];
        let local_bit = 1u64 << local_probe;
        if validity & local_bit == 0 {
            return Ok(None);
        }
        let level = self.level(cell).expect("validated cell level");
        let kept = kept_mask(level, validity);
        let entry_start = self.entry_f16_offsets[entry];
        let decode_local = |local: usize| -> anyhow::Result<Tile> {
            let rank = (kept & ((1u64 << local) - 1)).count_ones() as usize;
            self.decode_interior(entry_start + rank * self.probe_stride)
        };
        if kept & local_bit != 0 {
            return decode_local(local_probe).map(Some);
        }

        let mut kept_tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        let mut remaining = kept;
        while remaining != 0 {
            let local = remaining.trailing_zeros() as usize;
            kept_tiles[local] = Some(decode_local(local)?);
            remaining &= remaining - 1;
        }
        let reconstructed = match level {
            Level::L1 => reconstruct_l1_tile(&kept_tiles, local_probe, self.interior_texels)
                .unwrap_or_else(|| zero_tile(self.interior_texels)),
            Level::L2 => reconstruct_l2_tile(&kept_tiles, self.interior_texels)
                .expect("a valid L2 cell emits one representative mean tile"),
            Level::L0 => unreachable!("L0 keeps every valid target"),
        };
        Ok(Some(reconstructed))
    }

    /// Reconstruct every valid local probe for one entry while decoding its
    /// kept lattice once. Runtime-envelope validation uses this batch form so
    /// L1/L2 cells do not repeatedly decode the same stored tiles.
    pub(crate) fn reconstruct_entry_tiles(
        &self,
        cell: usize,
        entry: usize,
    ) -> anyhow::Result<[Option<Tile>; PROBES_PER_CELL]> {
        let range = self
            .entry_range(cell)
            .ok_or_else(|| anyhow::anyhow!("emitted delta view: cell {cell} is out of range"))?;
        anyhow::ensure!(
            range.contains(&entry),
            "emitted delta view: entry {entry} does not belong to cell {cell}"
        );
        let validity = self.valid_probe_masks[cell];
        let level = self.level(cell).expect("validated cell level");
        let kept = kept_mask(level, validity);
        let entry_start = self.entry_f16_offsets[entry];
        let decode_local = |local: usize| -> anyhow::Result<Tile> {
            let rank = (kept & ((1u64 << local) - 1)).count_ones() as usize;
            self.decode_interior(entry_start + rank * self.probe_stride)
        };
        let mut kept_tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        let mut remaining = kept;
        while remaining != 0 {
            let local = remaining.trailing_zeros() as usize;
            kept_tiles[local] = Some(decode_local(local)?);
            remaining &= remaining - 1;
        }
        let l2 = (level == Level::L2).then(|| {
            reconstruct_l2_tile(&kept_tiles, self.interior_texels)
                .expect("a valid L2 cell emits one representative mean tile")
        });
        let mut output: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        for local in 0..PROBES_PER_CELL {
            let bit = 1u64 << local;
            if validity & bit == 0 {
                continue;
            }
            output[local] = Some(if let Some(tile) = &kept_tiles[local] {
                tile.clone()
            } else {
                match level {
                    Level::L1 => reconstruct_l1_tile(&kept_tiles, local, self.interior_texels)
                        .unwrap_or_else(|| zero_tile(self.interior_texels)),
                    Level::L2 => l2.as_ref().expect("computed L2 tile").clone(),
                    Level::L0 => unreachable!("L0 keeps every valid target"),
                }
            });
        }
        Ok(output)
    }
}

/// Per-cell absolute error measured between dense L0 truth and the exact
/// represented output of a compacted delta section.
///
/// This is deliberately a per-section emitted-reconstruction diagnostic. It
/// does not model runtime light-selection weights, signs, or cross-section
/// composition.
#[derive(Clone, Copy, Debug, Default)]
#[cfg(test)]
pub(crate) struct EmittedReconstructionError {
    pub p95: f32,
    pub max: f32,
    pub texel_samples: u64,
}

/// Compare dense pre-compaction f16 truth with a section's final compact f16
/// payload, using the same represented semantics as the compose shaders.
///
/// `dense_subblocks` is the bake layout: one 64-probe record per CSR entry.
/// `compacted_subblocks` is the emitted variable-stride layout after final
/// post-smoothing `cell_levels` have been applied. The walk derives every
/// entry's compact offset from the preceding cells' actual kept masks, so mixed
/// L0/L1/L2 sections are measured without assuming a uniform entry stride.
/// L1 targets unsupported by the sparse kept-corner lattice reconstruct to
/// zero, matching the shader rather than disappearing from the score.
#[cfg(test)]
pub(crate) fn emitted_reconstruction_error_by_cell(
    affinity_offsets: &[u32],
    valid_probe_masks: &[u64],
    cell_levels: &[u8],
    dense_subblocks: &[u16],
    compacted_subblocks: &[u16],
    tile_dimension: u32,
    tile_border: u32,
) -> anyhow::Result<Vec<EmittedReconstructionError>> {
    let tile_dimension = tile_dimension as usize;
    let tile_border = tile_border as usize;
    let probe_stride = tile_dimension
        .checked_mul(tile_dimension)
        .and_then(|texels| texels.checked_mul(DELTA_TILE_TEXEL_F16_COUNT))
        .ok_or_else(|| anyhow::anyhow!("emitted reconstruction oracle: probe stride overflow"))?;
    let dense_entry_stride = PROBES_PER_CELL
        .checked_mul(probe_stride)
        .ok_or_else(|| anyhow::anyhow!("emitted reconstruction oracle: dense stride overflow"))?;
    let entry_count = affinity_offsets.last().copied().unwrap_or(0) as usize;
    let expected_dense_len = entry_count
        .checked_mul(dense_entry_stride)
        .ok_or_else(|| anyhow::anyhow!("emitted reconstruction oracle: dense length overflow"))?;
    anyhow::ensure!(
        dense_subblocks.len() == expected_dense_len,
        "emitted reconstruction oracle: dense payload has {} f16 values, expected {expected_dense_len}",
        dense_subblocks.len()
    );

    let emitted = EmittedDeltaSectionRef::new(
        affinity_offsets,
        valid_probe_masks,
        cell_levels,
        compacted_subblocks,
        tile_dimension as u32,
        tile_border as u32,
    )?;
    let decode_interior = |payload: &[u16], tile_start: usize| -> anyhow::Result<Tile> {
        let tile_end = tile_start.checked_add(probe_stride).ok_or_else(|| {
            anyhow::anyhow!("emitted reconstruction oracle: tile offset overflow")
        })?;
        anyhow::ensure!(
            tile_end <= payload.len(),
            "emitted reconstruction oracle: tile [{tile_start}..{tile_end}) exceeds payload length {}",
            payload.len()
        );
        let mut tile = Vec::with_capacity(emitted.interior_texels());
        for y in tile_border..tile_dimension - tile_border {
            for x in tile_border..tile_dimension - tile_border {
                let i = tile_start + (y * tile_dimension + x) * DELTA_TILE_TEXEL_F16_COUNT;
                tile.push(glam::Vec3::new(
                    crate::sh_bake::f16_bits_to_f32(payload[i]),
                    crate::sh_bake::f16_bits_to_f32(payload[i + 1]),
                    crate::sh_bake::f16_bits_to_f32(payload[i + 2]),
                ));
            }
        }
        Ok(tile)
    };

    let mut output = Vec::with_capacity(emitted.cell_count());
    for cell in 0..emitted.cell_count() {
        let validity = emitted.valid_probe_mask(cell).expect("cell is in range");
        let mut errors = Vec::new();
        for entry in emitted.entry_range(cell).expect("cell is in range") {
            let dense_entry_start = entry * dense_entry_stride;
            let mut valid = validity;
            while valid != 0 {
                let target = valid.trailing_zeros() as usize;
                valid &= valid - 1;
                let truth =
                    decode_interior(dense_subblocks, dense_entry_start + target * probe_stride)?;
                let reconstructed = emitted
                    .reconstruct_entry_tile(cell, entry, target)?
                    .expect("valid target reconstructs to a tile");
                errors.extend(
                    truth
                        .iter()
                        .zip(&reconstructed)
                        .map(|(truth, reconstructed)| {
                            (*reconstructed - *truth).abs().max_element()
                        }),
                );
            }
        }

        errors.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let max = errors.last().copied().unwrap_or(0.0);
        let p95 = if errors.is_empty() {
            0.0
        } else {
            let index = ((errors.len() - 1) as f32 * 0.95).round() as usize;
            errors[index.min(errors.len() - 1)]
        };
        output.push(EmittedReconstructionError {
            p95,
            max,
            texel_samples: errors.len() as u64,
        });
    }
    Ok(output)
}

fn affinity_dims_for_grid(grid_dimensions: [u32; 3], affinity_factor: u8) -> [u32; 3] {
    let factor = u32::from(affinity_factor);
    [
        grid_dimensions[0].div_ceil(factor),
        grid_dimensions[1].div_ceil(factor),
        grid_dimensions[2].div_ceil(factor),
    ]
}

fn valid_probe_mask_for_affinity_cell(
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
    cell_index: usize,
) -> u64 {
    let cell_index = cell_index as u32;
    let cell_x = cell_index % affinity_dims[0];
    let cell_y = (cell_index / affinity_dims[0]) % affinity_dims[1];
    let cell_z = cell_index / (affinity_dims[0] * affinity_dims[1]);
    let factor = u32::from(AFFINITY_FACTOR);
    let mut mask = 0u64;

    for local_z in 0..factor {
        for local_y in 0..factor {
            for local_x in 0..factor {
                let probe = [
                    cell_x * factor + local_x,
                    cell_y * factor + local_y,
                    cell_z * factor + local_z,
                ];
                if probe[0] >= base.grid_dimensions[0]
                    || probe[1] >= base.grid_dimensions[1]
                    || probe[2] >= base.grid_dimensions[2]
                {
                    continue;
                }
                let probe_index = probe[0] as usize
                    + probe[1] as usize * base.grid_dimensions[0] as usize
                    + probe[2] as usize
                        * base.grid_dimensions[0] as usize
                        * base.grid_dimensions[1] as usize;
                if base.probes[probe_index].validity != 0 {
                    let local = local_x + local_y * factor + local_z * factor * factor;
                    mask |= 1u64 << local;
                }
            }
        }
    }
    mask
}

fn payload_bytes(payload: &[u16]) -> u64 {
    u64::try_from(payload.len()).expect("payload length fits u64") * size_of::<u16>() as u64
}

fn csr_bytes(offsets: &[u32], lights: &[u32]) -> u64 {
    u64::try_from(offsets.len() + lights.len()).expect("CSR length fits u64")
        * size_of::<u32>() as u64
}

fn log_drop_summary(
    name: &str,
    stats: DropStats,
    input_payload_bytes: u64,
    retained_payload_bytes: u64,
    retained_csr_bytes: u64,
    normalized_absent: bool,
) {
    let emitted = if normalized_absent {
        "absent"
    } else {
        "present"
    };
    log::info!(
        "[Compiler] {name} delta drop: input {} entry/entries ({input_payload_bytes} raw payload bytes); \\
         retained {} and dropped {}; retained {retained_payload_bytes} raw payload bytes, \\
         {retained_csr_bytes} CSR bytes, largest accepted RGB bound [{:.6}, {:.6}, {:.6}], {emitted}",
        stats.input_entries,
        stats.retained_entries,
        stats.dropped_entries,
        stats.largest_accepted_bound[0],
        stats.largest_accepted_bound[1],
        stats.largest_accepted_bound[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level as LogLevel;
    use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
    use postretro_level_format::delta_sh_volumes::{
        DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL, valid_probe_mask_payload_f16_count,
    };
    use postretro_level_format::lightmap::f32_to_f16_bits;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };
    use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};
    use postretro_test_log_capture::LogCapture;

    use crate::delta_drop_policy::ScriptMutableDescriptorSlots;

    const TILE: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;
    const ENTRY_STRIDE: usize = PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE;

    fn block(rgb: [f32; 3]) -> Vec<u16> {
        let mut payload = Vec::with_capacity(ENTRY_STRIDE);
        for _ in 0..ENTRY_STRIDE / 4 {
            payload.extend(rgb.map(f32_to_f16_bits));
            payload.push(f32_to_f16_bits(1.0));
        }
        payload
    }

    fn dense_entry(seed: u16) -> Vec<u16> {
        let mut payload = Vec::with_capacity(ENTRY_STRIDE);
        for local_probe in 0..PROBES_PER_CELL {
            payload.extend(std::iter::repeat_n(
                seed.wrapping_add(local_probe as u16),
                DEFAULT_DELTA_PROBE_F16_STRIDE,
            ));
        }
        payload
    }

    fn base_with_valid_locals(valid_locals: &[usize]) -> OctahedralShVolumeSection {
        let mut base = OctahedralShVolumeSection::placeholder();
        base.grid_dimensions = [4, 4, 4];
        base.probes = vec![OctahedralShProbe::default(); PROBES_PER_CELL];
        for &local in valid_locals {
            base.probes[local].validity = 1;
        }
        base
    }

    fn base_with_x_cells(valid_locals: &[&[usize]]) -> OctahedralShVolumeSection {
        let cell_count = valid_locals.len();
        let dims = [cell_count * AFFINITY_FACTOR as usize, 4, 4];
        let mut base = OctahedralShVolumeSection::placeholder();
        base.grid_dimensions = [dims[0] as u32, dims[1] as u32, dims[2] as u32];
        base.probes = vec![OctahedralShProbe::default(); dims.iter().product()];
        for (cell_x, locals) in valid_locals.iter().enumerate() {
            for &local in *locals {
                let (local_x, local_y, local_z) =
                    postretro_level_format::sh_reconstruct::local_xyz(local);
                let x = cell_x * AFFINITY_FACTOR as usize + local_x;
                let probe = x + local_y * dims[0] + local_z * dims[0] * dims[1];
                base.probes[probe].validity = 1;
            }
        }
        base
    }

    fn indirect(entries: Vec<u32>, payload: Vec<u16>) -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [entries.len() as u32, 1, 1],
            tile_dimension: TILE,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX; entries.len()],
            cell_levels: vec![0u8; entries.len()],
            affinity_offsets: (0..=entries.len()).map(|index| index as u32).collect(),
            affinity_lights: entries,
            delta_subblocks: payload,
        }
    }

    fn direct(entries: Vec<u32>, payload: Vec<u16>) -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [entries.len() as u32, 1, 1],
            tile_dimension: TILE,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![u64::MAX; entries.len()],
            cell_levels: vec![0u8; entries.len()],
            affinity_offsets: (0..=entries.len()).map(|index| index as u32).collect(),
            affinity_lights: entries,
            delta_subblocks: payload,
        }
    }

    fn animated_direct(
        entries: Vec<u32>,
        payload: Vec<u16>,
    ) -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [entries.len() as u32, 1, 1],
            tile_dimension: TILE,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX; entries.len()],
            cell_levels: vec![0u8; entries.len()],
            affinity_offsets: (0..=entries.len()).map(|index| index as u32).collect(),
            affinity_lights: entries,
            delta_subblocks: payload,
        }
    }

    fn empty_indirect() -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![],
            valid_probe_masks: vec![],
            cell_levels: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    fn empty_direct() -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: vec![],
            cell_levels: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    fn empty_animated_direct() -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![],
            valid_probe_masks: vec![],
            cell_levels: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    #[test]
    fn post_bake_handoff_preserves_sections_and_owns_the_cap() {
        let config = DeltaSectionConfig {
            max_payload_bytes: 123,
        };
        let indirect = empty_indirect();
        let entity_shadow_lights = EntityShadowLightsSection {
            light_indices: vec![2, 7],
        };
        let direct = empty_direct();
        let animated_direct = empty_animated_direct();

        let handoff = PostBakeDeltaSections::new(
            config,
            Some(indirect.clone()),
            Some(entity_shadow_lights.clone()),
            Some(direct.clone()),
            Some(animated_direct.clone()),
        );

        assert_eq!(handoff.config, config);
        assert_eq!(handoff.indirect, Some(indirect));
        assert_eq!(handoff.entity_shadow_lights, Some(entity_shadow_lights));
        assert_eq!(handoff.direct, Some(direct));
        assert_eq!(handoff.animated_direct, Some(animated_direct));
    }

    #[test]
    fn delta_section_config_defaults_to_unconditional_256_mib_cap() {
        assert_eq!(
            DeltaSectionConfig::default().max_payload_bytes,
            256 * 1024 * 1024
        );
    }

    #[test]
    fn payload_over_64_mib_warns_without_changing_the_256_mib_cap() {
        let capture = LogCapture::start();
        let payload = DeltaPayloadBytes {
            indirect: 17,
            direct: 29,
            animated_direct: DIAGNOSTIC_MAX_PAYLOAD_BYTES + 1 - 46,
        };

        warn_if_over_authoring_budget(
            payload,
            DIAGNOSTIC_MAX_PAYLOAD_BYTES + 1,
            DEFAULT_MAX_PAYLOAD_BYTES,
        );

        capture.assert_logged_once(
            LogLevel::Warn,
            "SH delta authoring budget warning: id 27 17 bytes",
        );
        capture.assert_logged_once(LogLevel::Warn, "id 41 29 bytes");
        capture.assert_logged_once(LogLevel::Warn, "total 67108865 bytes");
        capture.assert_logged_once(LogLevel::Warn, "production cap");
    }

    #[test]
    fn policy_keeps_id27_empty_but_normalizes_empty_id45_to_absent() {
        let zero = block([0.0; 3]);
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            Some(indirect(vec![0], zero.clone())),
            None,
            None,
            Some(animated_direct(vec![0], zero)),
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(1))
            .expect("zero payloads fit the default cap");
        sections
            .apply_valid_probe_compaction(&base_with_valid_locals(&[]))
            .expect("compaction must preserve id 45's absent spelling");

        let indirect = sections
            .indirect
            .expect("id 27 preserves its empty spelling");
        assert!(indirect.affinity_lights.is_empty());
        assert_eq!(indirect.affinity_offsets, vec![0, 0]);
        assert!(sections.animated_direct.is_none());
    }

    #[test]
    fn policy_preserves_id41_selection_coverage_and_loader_shape() {
        let zero = block([0.0; 3]);
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            None,
            Some(EntityShadowLightsSection {
                light_indices: vec![42],
            }),
            Some(direct(vec![0, 0], [zero.clone(), zero].concat())),
            None,
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(0))
            .expect("covered zero direct section fits the default cap");

        let direct = sections.direct.expect("id 41 remains present");
        assert_eq!(direct.affinity_lights, vec![0]);
        assert!(crate::pack::direct_sh_delta_covers_selection(&direct, 1));
        assert!(crate::pack::direct_sh_delta_has_valid_csr_shape(&direct));
        let decoded = DirectShDeltaVolumesSection::from_bytes(&direct.to_bytes())
            .expect("retained id 41 uses the existing loader format");
        assert_eq!(decoded, direct);
    }

    #[test]
    fn policy_output_is_deterministic_and_retains_nonzero_payload_pairs() {
        let zero = block([0.0; 3]);
        let nonzero = block([0.25, 0.0, 0.0]);
        let make_sections = || {
            PostBakeDeltaSections::new(
                DeltaSectionConfig::default(),
                Some(indirect(
                    vec![0, 0, 0],
                    [zero.clone(), nonzero.clone(), zero.clone()].concat(),
                )),
                None,
                None,
                None,
            )
        };
        let mut first = make_sections();
        let mut second = make_sections();
        let mutable = ScriptMutableDescriptorSlots::empty(1);
        first
            .apply_exact_zero_drop_policy(&mutable)
            .expect("small payload fits cap");
        second
            .apply_exact_zero_drop_policy(&mutable)
            .expect("small payload fits cap");

        let first = first.indirect.expect("id 27 is emitted");
        let second = second.indirect.expect("id 27 is emitted");
        assert_eq!(first.affinity_offsets, vec![0, 0, 1, 1]);
        assert_eq!(first.affinity_lights, vec![0]);
        assert_eq!(first.delta_subblocks, nonzero);
        assert_eq!(first.to_bytes(), second.to_bytes());
    }

    #[test]
    fn cap_rejection_reports_every_delta_section_before_packing() {
        let nonzero = block([0.25, 0.0, 0.0]);
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig {
                max_payload_bytes: payload_bytes(&nonzero) - 1,
            },
            Some(indirect(vec![0], nonzero)),
            None,
            None,
            None,
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(1))
            .expect("dropping does not enforce the cap before compaction");
        let error = sections
            .enforce_payload_cap()
            .expect_err("the retained raw payload is over the explicit cap");
        let message = error.to_string();
        assert!(message.contains("id 27"));
        assert!(message.contains("id 41"));
        assert!(message.contains("id 45"));
        assert!(message.contains("exceeds cap"));
    }

    #[test]
    fn valid_probe_compaction_copies_only_id34_valid_tiles_in_x_fastest_order() {
        let mut section = direct(vec![0, 1], [dense_entry(100), dense_entry(1_000)].concat());
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 2];
        section.valid_probe_masks = vec![u64::MAX];
        let base = base_with_valid_locals(&[1, 6, 32]);

        let compacted = compact_direct_valid_probes(&section, &base)
            .expect("a dense post-drop direct section should compact");
        let expected_mask = (1u64 << 1) | (1u64 << 6) | (1u64 << 32);
        assert_eq!(compacted.valid_probe_masks, vec![expected_mask]);
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * 3 * DEFAULT_DELTA_PROBE_F16_STRIDE,
            "two CSR entries retain exactly the three id-34-valid tiles"
        );

        for (rank, local_probe) in [1usize, 6, 32].into_iter().enumerate() {
            let first_start = rank * DEFAULT_DELTA_PROBE_F16_STRIDE;
            let second_start = (3 + rank) * DEFAULT_DELTA_PROBE_F16_STRIDE;
            assert!(
                compacted.delta_subblocks
                    [first_start..first_start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 100 + local_probe as u16)
            );
            assert!(
                compacted.delta_subblocks
                    [second_start..second_start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 1_000 + local_probe as u16)
            );
        }
    }

    #[test]
    fn animated_direct_valid_probe_compaction_copies_only_id34_valid_tiles_in_x_fastest_order() {
        let mut section =
            animated_direct(vec![0, 0], [dense_entry(100), dense_entry(1_000)].concat());
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 2];
        section.valid_probe_masks = vec![u64::MAX];
        let base = base_with_valid_locals(&[1, 6, 32]);

        let compacted = compact_animated_direct_valid_probes(&section, &base)
            .expect("a dense post-drop animated-direct section should compact");
        let expected_mask = (1u64 << 1) | (1u64 << 6) | (1u64 << 32);
        assert_eq!(
            compacted.animation_descriptor_indices,
            section.animation_descriptor_indices
        );
        assert_eq!(compacted.valid_probe_masks, vec![expected_mask]);
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * 3 * DEFAULT_DELTA_PROBE_F16_STRIDE,
            "two CSR entries retain exactly the three id-34-valid tiles"
        );

        for (rank, local_probe) in [1usize, 6, 32].into_iter().enumerate() {
            let first_start = rank * DEFAULT_DELTA_PROBE_F16_STRIDE;
            let second_start = (3 + rank) * DEFAULT_DELTA_PROBE_F16_STRIDE;
            assert!(
                compacted.delta_subblocks
                    [first_start..first_start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 100 + local_probe as u16)
            );
            assert!(
                compacted.delta_subblocks
                    [second_start..second_start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 1_000 + local_probe as u16)
            );
        }
    }

    #[test]
    fn animated_direct_compaction_keeps_script_mutable_zero_length_entry() {
        let mut section = animated_direct(vec![0], block([0.0; 3]));
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 1];
        let mut mutable = ScriptMutableDescriptorSlots::empty(1);
        mutable.animated_direct[0] = true;
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            None,
            None,
            None,
            Some(section),
        );

        sections
            .apply_exact_zero_drop_policy(&mutable)
            .expect("a script-mutable zero entry must survive drop");
        sections
            .apply_valid_probe_compaction(&base_with_valid_locals(&[]))
            .expect("an all-invalid retained entry has a zero-length compact payload");

        let compacted = sections
            .animated_direct
            .expect("a retained script-mutable entry keeps id 45 present");
        assert_eq!(compacted.affinity_offsets, vec![0, 1]);
        assert_eq!(compacted.affinity_lights, vec![0]);
        assert_eq!(compacted.valid_probe_masks, vec![0]);
        assert!(compacted.delta_subblocks.is_empty());
    }

    #[test]
    fn animated_direct_drop_then_compaction_caps_the_compacted_payload() {
        let zero = block([0.0; 3]);
        let nonzero = block([0.25, 0.0, 0.0]);
        let compacted_bytes =
            u64::try_from(2 * DEFAULT_DELTA_PROBE_F16_STRIDE * size_of::<u16>()).unwrap();
        let mut section = animated_direct(vec![0, 0], [zero, nonzero].concat());
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 2];
        section.valid_probe_masks = vec![u64::MAX];
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig {
                max_payload_bytes: compacted_bytes - 1,
            },
            None,
            None,
            None,
            Some(section),
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(1))
            .expect("the payload cap runs after id-45 compaction");
        let dense_after_drop = sections.animated_direct.as_ref().unwrap();
        assert_eq!(dense_after_drop.affinity_lights, vec![0]);
        assert_eq!(dense_after_drop.delta_subblocks.len(), ENTRY_STRIDE);

        sections
            .apply_valid_probe_compaction(&base_with_valid_locals(&[0, 2]))
            .expect("id-45 compaction follows exact-zero dropping");
        let compacted = sections.animated_direct.as_ref().unwrap();
        assert_eq!(compacted.affinity_lights, vec![0]);
        assert_eq!(compacted.valid_probe_masks, vec![(1u64 << 0) | (1u64 << 2)]);
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * DEFAULT_DELTA_PROBE_F16_STRIDE
        );

        let error = sections
            .enforce_payload_cap()
            .expect_err("the cap must count id-45's compacted survivor only");
        assert!(error.to_string().contains("id 45"));
        assert!(error.to_string().contains("by 1 bytes"));
    }

    #[test]
    fn indirect_valid_probe_compaction_copies_only_id34_valid_tiles_in_x_fastest_order() {
        let mut section = indirect(vec![0, 0], [dense_entry(100), dense_entry(1_000)].concat());
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 2];
        section.valid_probe_masks = vec![u64::MAX];
        let base = base_with_valid_locals(&[1, 6, 32]);

        let compacted = compact_indirect_valid_probes(&section, &base)
            .expect("a dense post-drop indirect section should compact");
        let expected_mask = (1u64 << 1) | (1u64 << 6) | (1u64 << 32);
        assert_eq!(compacted.valid_probe_masks, vec![expected_mask]);
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * 3 * DEFAULT_DELTA_PROBE_F16_STRIDE,
            "two CSR entries retain exactly the three id-34-valid tiles"
        );

        for (rank, local_probe) in [1usize, 6, 32].into_iter().enumerate() {
            let first_start = rank * DEFAULT_DELTA_PROBE_F16_STRIDE;
            let second_start = (3 + rank) * DEFAULT_DELTA_PROBE_F16_STRIDE;
            assert!(
                compacted.delta_subblocks
                    [first_start..first_start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 100 + local_probe as u16)
            );
            assert!(
                compacted.delta_subblocks
                    [second_start..second_start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 1_000 + local_probe as u16)
            );
        }
    }

    #[test]
    fn direct_drop_then_compaction_caps_the_compacted_payload_and_preserves_drop_set() {
        let zero = block([0.0; 3]);
        let nonzero = block([0.25, 0.0, 0.0]);
        let compacted_bytes =
            u64::try_from(2 * DEFAULT_DELTA_PROBE_F16_STRIDE * size_of::<u16>()).unwrap();
        let mut direct_section = direct(vec![0, 0], [zero, nonzero].concat());
        direct_section.affinity_dims = [1, 1, 1];
        direct_section.affinity_offsets = vec![0, 2];
        direct_section.valid_probe_masks = vec![u64::MAX];
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig {
                max_payload_bytes: compacted_bytes - 1,
            },
            None,
            None,
            Some(direct_section),
            None,
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(0))
            .expect("the cap is intentionally delayed until compaction");
        let dense_after_drop = sections.direct.as_ref().unwrap();
        assert_eq!(dense_after_drop.affinity_lights, vec![0]);
        assert_eq!(dense_after_drop.delta_subblocks.len(), ENTRY_STRIDE);

        sections
            .apply_valid_probe_compaction(&base_with_valid_locals(&[0, 2]))
            .expect("valid-probe compaction should follow exact-zero dropping");
        let compacted = sections.direct.as_ref().unwrap();
        assert_eq!(compacted.affinity_lights, vec![0]);
        assert_eq!(compacted.valid_probe_masks, vec![(1u64 << 0) | (1u64 << 2)]);
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * DEFAULT_DELTA_PROBE_F16_STRIDE
        );

        let error = sections
            .enforce_payload_cap()
            .expect_err("the cap must count the compacted survivor only");
        let message = error.to_string();
        assert!(message.contains("id 27"));
        assert!(message.contains("id 41"));
        assert!(message.contains("id 45"));
        assert!(message.contains("by 1 bytes"));
    }

    #[test]
    fn indirect_drop_then_compaction_caps_the_compacted_payload_and_preserves_drop_set() {
        let zero = block([0.0; 3]);
        let nonzero = block([0.25, 0.0, 0.0]);
        let compacted_bytes =
            u64::try_from(2 * DEFAULT_DELTA_PROBE_F16_STRIDE * size_of::<u16>()).unwrap();
        let mut indirect_section = indirect(vec![0, 0], [zero, nonzero].concat());
        indirect_section.affinity_dims = [1, 1, 1];
        indirect_section.affinity_offsets = vec![0, 2];
        indirect_section.valid_probe_masks = vec![u64::MAX];
        let mut sections = PostBakeDeltaSections::new(
            DeltaSectionConfig {
                max_payload_bytes: compacted_bytes - 1,
            },
            Some(indirect_section),
            None,
            None,
            None,
        );

        sections
            .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(1))
            .expect("the cap is intentionally delayed until compaction");
        let dense_after_drop = sections.indirect.as_ref().unwrap();
        assert_eq!(dense_after_drop.affinity_lights, vec![0]);
        assert_eq!(dense_after_drop.delta_subblocks.len(), ENTRY_STRIDE);

        sections
            .apply_valid_probe_compaction(&base_with_valid_locals(&[0, 2]))
            .expect("valid-probe compaction should follow exact-zero dropping");
        let compacted = sections.indirect.as_ref().unwrap();
        assert_eq!(compacted.affinity_lights, vec![0]);
        assert_eq!(compacted.valid_probe_masks, vec![(1u64 << 0) | (1u64 << 2)]);
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * DEFAULT_DELTA_PROBE_F16_STRIDE
        );

        let error = sections
            .enforce_payload_cap()
            .expect_err("the cap must count the compacted survivor only");
        let message = error.to_string();
        assert!(message.contains("id 27"));
        assert!(message.contains("id 41"));
        assert!(message.contains("id 45"));
        assert!(message.contains("by 1 bytes"));
    }

    /// A dense delta entry whose named local probe tiles carry a constant RGB
    /// (alpha 1.0); every other probe tile is left zero. Lets an L2 test pin
    /// known distinct per-probe values.
    fn dense_entry_with_probe_rgb(values: &[(usize, [f32; 3])]) -> Vec<u16> {
        let mut payload = vec![0u16; ENTRY_STRIDE];
        let alpha = f32_to_f16_bits(1.0);
        for &(local, rgb) in values {
            let base = local * DEFAULT_DELTA_PROBE_F16_STRIDE;
            for texel in 0..DEFAULT_DELTA_PROBE_F16_STRIDE / 4 {
                let i = base + texel * 4;
                payload[i] = f32_to_f16_bits(rgb[0]);
                payload[i + 1] = f32_to_f16_bits(rgb[1]);
                payload[i + 2] = f32_to_f16_bits(rgb[2]);
                payload[i + 3] = alpha;
            }
        }
        payload
    }

    fn dense_entry_from_rgb(mut rgb: impl FnMut(usize) -> [f32; 3]) -> Vec<u16> {
        let values: Vec<_> = (0..PROBES_PER_CELL)
            .map(|local| (local, rgb(local)))
            .collect();
        dense_entry_with_probe_rgb(&values)
    }

    /// P1 / AC1: an L1 cell stores only its valid corner tiles — fewer than the
    /// full valid set — while `valid_probe_masks` still carries full validity and
    /// the payload length matches the level-aware wire identity.
    #[test]
    fn l1_coarsening_stores_only_valid_corner_tiles() {
        // Validity spans corners {0, 3} and non-corner interiors {1, 6}.
        let base = base_with_valid_locals(&[0, 1, 3, 6]);
        let validity = (1u64 << 0) | (1u64 << 1) | (1u64 << 3) | (1u64 << 6);
        let mut section = direct(vec![0], dense_entry(100));
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 1];
        section.cell_levels = vec![Level::L1.to_u8()];

        let compacted = compact_direct_valid_probes(&section, &base).expect("an L1 cell compacts");

        // Emitted `valid_probe_masks` stays FULL validity; the kept set is
        // derived from (level, validity) via `kept_mask`, never stored.
        assert_eq!(compacted.valid_probe_masks, vec![validity]);
        assert_eq!(compacted.cell_levels, vec![Level::L1.to_u8()]);

        // Only the valid corners {0, 3} are stored: 2 tiles < 4 valid probes.
        assert_eq!(
            compacted.delta_subblocks.len(),
            2 * DEFAULT_DELTA_PROBE_F16_STRIDE
        );
        assert!(2 < validity.count_ones());

        // The two stored tiles are exactly corners 0 and 3 in x-fastest order.
        for (rank, local) in [0usize, 3].into_iter().enumerate() {
            let start = rank * DEFAULT_DELTA_PROBE_F16_STRIDE;
            assert!(
                compacted.delta_subblocks[start..start + DEFAULT_DELTA_PROBE_F16_STRIDE]
                    .iter()
                    .all(|&half| half == 100 + local as u16)
            );
        }

        // Payload length equals the level-aware wire identity.
        let expected = valid_probe_mask_payload_f16_count(
            &compacted.affinity_offsets,
            &compacted.valid_probe_masks,
            &compacted.cell_levels,
            section.delta_probe_f16_stride(),
        )
        .expect("the level-aware identity is defined for an L1 cell");
        assert_eq!(compacted.delta_subblocks.len(), expected);
    }

    /// P2: an L2 cell emits exactly one synthesized tile at the representative
    /// slot, equal to `reconstruct_l2_tile` over the entry's valid probe tiles —
    /// the brick mean, not any single copied probe.
    #[test]
    fn l2_coarsening_stores_synthesized_brick_mean_tile() {
        let values = [
            (1usize, [0.1f32, 0.2, 0.3]),
            (5, [0.4, 0.5, 0.6]),
            (9, [0.7, 0.8, 0.9]),
        ];
        let base = base_with_valid_locals(&[1, 5, 9]);
        let validity = (1u64 << 1) | (1u64 << 5) | (1u64 << 9);
        let mut section = direct(vec![0], dense_entry_with_probe_rgb(&values));
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 1];
        section.cell_levels = vec![Level::L2.to_u8()];

        let compacted = compact_direct_valid_probes(&section, &base).expect("an L2 cell compacts");

        assert_eq!(compacted.valid_probe_masks, vec![validity]);
        // Exactly one stored tile (the representative slot = lowest valid bit).
        assert_eq!(
            compacted.delta_subblocks.len(),
            DEFAULT_DELTA_PROBE_F16_STRIDE,
            "L2 stores exactly one tile"
        );

        // Golden brick mean via the shared reconstruction, over the same tiles.
        let tile_texels = DEFAULT_DELTA_PROBE_F16_STRIDE / 4;
        let mut valid_tiles: [Option<Vec<glam::Vec3>>; PROBES_PER_CELL] =
            std::array::from_fn(|_| None);
        for &(local, rgb) in &values {
            valid_tiles[local] = Some(vec![glam::Vec3::from(rgb); tile_texels]);
        }
        let golden = reconstruct_l2_tile(&valid_tiles, tile_texels)
            .expect("a non-empty valid set has a brick mean");

        // Decode the emitted representative tile and compare to the mean.
        for texel in 0..tile_texels {
            let i = texel * 4;
            let rgb = glam::Vec3::new(
                crate::sh_bake::f16_bits_to_f32(compacted.delta_subblocks[i]),
                crate::sh_bake::f16_bits_to_f32(compacted.delta_subblocks[i + 1]),
                crate::sh_bake::f16_bits_to_f32(compacted.delta_subblocks[i + 2]),
            );
            assert!(
                (rgb - golden[texel]).abs().max_element() < 1e-2,
                "emitted L2 tile must equal reconstruct_l2_tile: got {rgb:?} expected {:?}",
                golden[texel]
            );
        }
        // The mean [0.4, 0.5, 0.6] is not any single copied probe value.
        let emitted0 = glam::Vec3::new(
            crate::sh_bake::f16_bits_to_f32(compacted.delta_subblocks[0]),
            crate::sh_bake::f16_bits_to_f32(compacted.delta_subblocks[1]),
            crate::sh_bake::f16_bits_to_f32(compacted.delta_subblocks[2]),
        );
        for &(_, rgb) in &values {
            let probe = glam::Vec3::from(rgb);
            if (probe - glam::Vec3::new(0.4, 0.5, 0.6)).abs().max_element() > 1e-3 {
                assert!(
                    (emitted0 - probe).abs().max_element() > 1e-3,
                    "the L2 tile must be the mean, not a copied probe {probe:?}"
                );
            }
        }
    }

    /// AC2: a section this producer emits with an L2 cell round-trips through the
    /// existing loader unchanged.
    #[test]
    fn l2_produced_section_round_trips() {
        let values = [
            (1usize, [0.1f32, 0.2, 0.3]),
            (5, [0.4, 0.5, 0.6]),
            (9, [0.7, 0.8, 0.9]),
        ];
        let base = base_with_valid_locals(&[1, 5, 9]);
        let mut section = direct(vec![0], dense_entry_with_probe_rgb(&values));
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 1];
        section.cell_levels = vec![Level::L2.to_u8()];

        let compacted = compact_direct_valid_probes(&section, &base).expect("an L2 cell compacts");
        let decoded = DirectShDeltaVolumesSection::from_bytes(&compacted.to_bytes())
            .expect("a producer-emitted L2 section uses the existing loader format");
        assert_eq!(decoded, compacted);
    }

    #[test]
    fn emitted_oracle_walks_mixed_level_variable_entry_offsets() {
        let all_locals: Vec<usize> = (0..PROBES_PER_CELL).collect();
        let base = base_with_x_cells(&[&all_locals, &all_locals, &all_locals]);
        let dense_l0 = dense_entry_from_rgb(|local| [local as f32, 0.0, 0.0]);
        let dense_l1 = dense_entry_from_rgb(|local| {
            let (x, y, z) = postretro_level_format::sh_reconstruct::local_xyz(local);
            [x as f32, y as f32, z as f32]
        });
        let dense_l2 = block([3.0, 3.0, 3.0]);
        let dense_payload = [dense_l0, dense_l1, dense_l2].concat();
        let mut section = direct(vec![0, 1, 2], dense_payload.clone());
        section.affinity_dims = [3, 1, 1];
        section.cell_levels = vec![Level::L0.to_u8(), Level::L1.to_u8(), Level::L2.to_u8()];

        let compacted =
            compact_direct_valid_probes(&section, &base).expect("mixed L0/L1/L2 section compacts");
        assert_eq!(
            compacted.delta_subblocks.len(),
            (64 + 8 + 1) * DEFAULT_DELTA_PROBE_F16_STRIDE
        );

        let errors = emitted_reconstruction_error_by_cell(
            &compacted.affinity_offsets,
            &compacted.valid_probe_masks,
            &compacted.cell_levels,
            &dense_payload,
            &compacted.delta_subblocks,
            compacted.tile_dimension,
            compacted.tile_border,
        )
        .expect("oracle resolves each variable-stride entry");
        assert_eq!(errors.len(), 3);
        for (cell, error) in errors.iter().enumerate() {
            assert!(
                error.max < 1.0e-6,
                "cell {cell} reconstructed through the wrong compact offset: {error:?}"
            );
            assert!(error.p95 < 1.0e-6);
            assert_eq!(error.texel_samples, 64 * 16);
        }

        let emitted = EmittedDeltaSectionRef::from_direct(&compacted)
            .expect("producer output forms an emitted view");
        let l2 = emitted
            .reconstruct_entry_tile(2, 2, 63)
            .expect("L2 entry offset is valid")
            .expect("local 63 is valid");
        assert!((l2[0] - glam::Vec3::splat(3.0)).abs().max_element() < 1.0e-6);
    }

    // Regression: Stress-Warren 1.25 m cell 5213 exposed sparse-L1 targets
    // that the candidate scorer omitted while the compose shader wrote zero.
    #[test]
    fn emitted_oracle_scores_sparse_l1_zero_fallback() {
        let base = base_with_valid_locals(&[0, 7]);
        let dense_payload =
            dense_entry_with_probe_rgb(&[(0, [0.5, 0.5, 0.5]), (7, [1.0, 1.0, 1.0])]);
        let mut section = direct(vec![0], dense_payload.clone());
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 1];
        section.cell_levels = vec![Level::L1.to_u8()];

        let texels = 16;
        let mut candidate_tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        candidate_tiles[0] = Some(vec![glam::Vec3::splat(0.5); texels]);
        candidate_tiles[7] = Some(vec![glam::Vec3::ONE; texels]);
        let weights = vec![1.0; texels];
        let candidate = crate::sh_analyze::level_errors(
            &candidate_tiles,
            crate::sh_analyze::LevelKind::L1,
            texels,
            4,
            &weights,
        );
        assert!(candidate.max.abs() < 1.0e-6);
        assert_eq!(candidate.texel_samples, texels as u64);

        let compacted =
            compact_direct_valid_probes(&section, &base).expect("sparse L1 section compacts");
        let emitted = EmittedDeltaSectionRef::from_direct(&compacted)
            .expect("sparse L1 output forms an emitted view");
        let unsupported = emitted
            .reconstruct_entry_tile(0, 0, 7)
            .expect("sparse L1 lookup is valid")
            .expect("local 7 is base-valid");
        assert!(
            unsupported
                .iter()
                .all(|value| value.abs().max_element() < 1.0e-6)
        );

        let errors = emitted_reconstruction_error_by_cell(
            &compacted.affinity_offsets,
            &compacted.valid_probe_masks,
            &compacted.cell_levels,
            &dense_payload,
            &compacted.delta_subblocks,
            compacted.tile_dimension,
            compacted.tile_border,
        )
        .expect("oracle measures the represented sparse-L1 fallback");
        assert_eq!(errors.len(), 1);
        assert!((errors[0].max - 1.0).abs() < 1.0e-6);
        assert!((errors[0].p95 - 1.0).abs() < 1.0e-6);
        assert_eq!(errors[0].texel_samples, 2 * texels as u64);
    }

    /// The all-L0 path is byte-identical to the pre-coarsening "copy every valid
    /// tile" behavior for a mixed, multi-cell section.
    #[test]
    fn all_l0_mixed_section_is_byte_identical_to_copy_all_valid() {
        // Two affinity cells along x: cell 0 (x 0..3), cell 1 (x 4..7).
        let mut base = OctahedralShVolumeSection::placeholder();
        base.grid_dimensions = [8, 4, 4];
        base.probes = vec![OctahedralShProbe::default(); 8 * 4 * 4];
        let global = |cell_x_base: u32, local: usize| -> usize {
            let (lx, ly, lz) = (local % 4, (local / 4) % 4, local / 16);
            (cell_x_base as usize + lx) + ly * 8 + lz * 32
        };
        for &local in &[1usize, 6, 32] {
            base.probes[global(0, local)].validity = 1;
        }
        for &local in &[0usize, 3] {
            base.probes[global(4, local)].validity = 1;
        }

        let payload = [dense_entry(100), dense_entry(1_000)].concat();
        let mut section = direct(vec![0, 0], payload);
        section.affinity_dims = [2, 1, 1];
        section.affinity_offsets = vec![0, 1, 2];
        section.valid_probe_masks = vec![u64::MAX; 2];
        section.cell_levels = vec![Level::L0.to_u8(); 2];

        let compacted =
            compact_direct_valid_probes(&section, &base).expect("an all-L0 section compacts");

        assert_eq!(
            compacted.valid_probe_masks,
            vec![
                (1u64 << 1) | (1u64 << 6) | (1u64 << 32),
                (1u64 << 0) | (1u64 << 3),
            ]
        );

        // Hand-built expected = copy every valid tile, x-fastest, per entry.
        let entry0 = dense_entry(100);
        let entry1 = dense_entry(1_000);
        let mut expected = Vec::new();
        for &local in &[1usize, 6, 32] {
            let s = local * DEFAULT_DELTA_PROBE_F16_STRIDE;
            expected.extend_from_slice(&entry0[s..s + DEFAULT_DELTA_PROBE_F16_STRIDE]);
        }
        for &local in &[0usize, 3] {
            let s = local * DEFAULT_DELTA_PROBE_F16_STRIDE;
            expected.extend_from_slice(&entry1[s..s + DEFAULT_DELTA_PROBE_F16_STRIDE]);
        }
        assert_eq!(compacted.delta_subblocks, expected);
    }

    #[test]
    fn id41_only_policy_keeps_ids27_and45_byte_identical_to_uniform_l0() {
        let all_valid = (0..PROBES_PER_CELL).collect::<Vec<_>>();
        let base = base_with_x_cells(&[&all_valid, &all_valid]);
        let payload = [block([0.25, 0.5, 0.75]), block([0.5, 0.25, 0.125])].concat();

        let baseline_indirect = indirect(vec![0, 0], payload.clone());
        let baseline_direct = direct(vec![0, 0], payload.clone());
        let baseline_animated = animated_direct(vec![0, 0], payload.clone());
        let mut uniform = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            Some(baseline_indirect.clone()),
            None,
            Some(baseline_direct.clone()),
            Some(baseline_animated.clone()),
        );

        let mut candidate_indirect = baseline_indirect;
        candidate_indirect.cell_levels = vec![Level::L1.to_u8(), Level::L2.to_u8()];
        let mut candidate_direct = baseline_direct;
        candidate_direct.cell_levels = vec![Level::L1.to_u8(), Level::L2.to_u8()];
        let mut candidate_animated = baseline_animated;
        candidate_animated.cell_levels = vec![Level::L1.to_u8(), Level::L2.to_u8()];
        let mut id41_only = PostBakeDeltaSections::new(
            DeltaSectionConfig::default(),
            Some(candidate_indirect),
            None,
            Some(candidate_direct),
            Some(candidate_animated),
        );

        id41_only.enforce_id41_only_coarsening_policy();
        assert_eq!(
            id41_only.indirect.as_ref().unwrap().cell_levels,
            vec![Level::L0.to_u8(); 2]
        );
        assert_eq!(
            id41_only.animated_direct.as_ref().unwrap().cell_levels,
            vec![Level::L0.to_u8(); 2]
        );
        assert_eq!(
            id41_only.direct.as_ref().unwrap().cell_levels,
            vec![Level::L1.to_u8(), Level::L2.to_u8()]
        );
        assert_eq!(
            i16::from(id41_only.direct.as_ref().unwrap().cell_levels[0])
                - i16::from(id41_only.direct.as_ref().unwrap().cell_levels[1]),
            -1,
            "participating adjacent id-41 cells preserve the I5 level bound"
        );

        uniform
            .apply_valid_probe_compaction(&base)
            .expect("uniform sections compact");
        id41_only
            .apply_valid_probe_compaction(&base)
            .expect("id-41-only sections compact");

        assert_eq!(
            id41_only.indirect.as_ref().unwrap().to_bytes(),
            uniform.indirect.as_ref().unwrap().to_bytes(),
            "id 27 must retain the exact uniform-L0 representation"
        );
        assert_eq!(
            id41_only.animated_direct.as_ref().unwrap().to_bytes(),
            uniform.animated_direct.as_ref().unwrap().to_bytes(),
            "id 45 must retain the exact uniform-L0 representation"
        );
        assert_ne!(
            id41_only.direct.as_ref().unwrap().to_bytes(),
            uniform.direct.as_ref().unwrap().to_bytes(),
            "id 41 remains eligible to coarsen"
        );
        assert!(
            id41_only.direct.as_ref().unwrap().delta_subblocks.len()
                < uniform.direct.as_ref().unwrap().delta_subblocks.len()
        );
    }

    /// P12: the payload cap still hard-errors exactly once on a coarsened section,
    /// with no drop-to-fit / coarsening retry.
    #[test]
    fn coarsened_payload_still_hard_fails_the_cap_without_retry() {
        let base = base_with_valid_locals(&[0, 1, 3, 6]);
        let mut section = indirect(vec![0], dense_entry(100));
        section.affinity_dims = [1, 1, 1];
        section.affinity_offsets = vec![0, 1];
        section.cell_levels = vec![Level::L1.to_u8()];

        let compacted =
            compact_indirect_valid_probes(&section, &base).expect("an L1 cell compacts");
        let compacted_bytes = payload_bytes(&compacted.delta_subblocks);
        let sections = PostBakeDeltaSections::new(
            DeltaSectionConfig {
                max_payload_bytes: compacted_bytes - 1,
            },
            Some(compacted),
            None,
            None,
            None,
        );

        let error = sections
            .enforce_payload_cap()
            .expect_err("a coarsened payload over the cap hard-fails");
        assert!(error.to_string().contains("exceeds cap"));
        // No retry: the section is untouched and a second call fails identically.
        assert!(sections.indirect.is_some());
        let error_again = sections
            .enforce_payload_cap()
            .expect_err("the cap does not coarsen-to-fit; it fails again");
        assert_eq!(error.to_string(), error_again.to_string());
    }
}
