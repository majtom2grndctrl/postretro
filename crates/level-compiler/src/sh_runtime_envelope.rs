//! Runtime-safe bake-time repair for coarsened direct-SH deltas (PRL id 41).
//!
//! Each id-41 CSR entry is independently weighted in `[0, 1]` at runtime.  A
//! stored-unit sum can therefore hide reconstruction errors through
//! cancellation.  This pass instead sums the absolute RGB residual of every
//! entry (the triangle envelope), then normalizes that cancellation-free bound
//! by dense reference illumination and the classifier darkness floor.

use glam::Vec3;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{
    DELTA_TILE_TEXEL_F16_COUNT, DeltaShVolumesSection, PROBES_PER_CELL,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::sh_reconstruct::{
    Level, Tile, kept_mask, reconstruct_l1_tile, reconstruct_l2_tile, zero_tile,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::delta_drop_policy::ScriptMutableDescriptorSlots;
use crate::delta_sections::{
    EmittedDeltaSectionRef, PostBakeDeltaSections, compact_direct_valid_probes,
};
use crate::sh_analyze::{
    AnalyzeInputs, DeltaView, MagnitudeStats, build_brick_tiles, classifier_darkness_floor,
    tile_magnitude,
};
use crate::sh_coarsen::{CoarsenParams, DeltaSectionsRef};

const AF: usize = AFFINITY_FACTOR as usize;

#[path = "sh_runtime_envelope_scoring.rs"]
mod scoring;
use scoring::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EnvelopeStats {
    pub p95: f32,
    pub max: f32,
    pub samples: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MutableForcedL0Cost {
    pub section_id: u32,
    pub affected_cells: u64,
    pub affected_entries: u64,
    pub current_payload_bytes: u64,
    pub forced_l0_payload_bytes: u64,
    pub uniform_l0_payload_bytes: u64,
    pub forced_l0_retained_ratio: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RuntimeEnvelopeReport {
    pub dense_reference_map_p95: f32,
    pub darkness_floor: f32,
    pub failures_before_repair: u64,
    pub failures_after_repair: u64,
    pub selected_l0_restores: u64,
    pub smoothing_refinements: u64,
    pub incremental_payload_bytes: u64,
    pub indirect_mutable_cost: MutableForcedL0Cost,
    pub animated_direct_mutable_cost: MutableForcedL0Cost,
}

/// Apply the id-41 runtime-weight envelope after independent classification
/// and before the compiler's one real valid-probe compaction pass.
pub(crate) fn apply_runtime_safe_envelope(
    base: &OctahedralShVolumeSection,
    base_direct: Option<&DirectShVolumeSection>,
    sections: &mut PostBakeDeltaSections,
    mutable: &ScriptMutableDescriptorSlots,
    params: &CoarsenParams,
) -> anyhow::Result<RuntimeEnvelopeReport> {
    let valid_masks = base_valid_probe_masks(base)?;
    let indirect_mutable_cost = sections.indirect.as_ref().map_or_else(
        || MutableForcedL0Cost {
            section_id: 27,
            ..Default::default()
        },
        |section| mutable_indirect_cost(section, &valid_masks, mutable),
    );
    let animated_direct_mutable_cost = sections.animated_direct.as_ref().map_or_else(
        || MutableForcedL0Cost {
            section_id: 45,
            ..Default::default()
        },
        |section| mutable_animated_direct_cost(section, &valid_masks),
    );

    let magnitudes = dense_reference_magnitudes(
        base,
        base_direct,
        DeltaSectionsRef {
            indirect: sections.indirect.as_ref(),
            direct: sections.direct.as_ref(),
            anim_direct: sections.animated_direct.as_ref(),
        },
    )?;
    let participating_magnitudes: Vec<f32> = magnitudes
        .iter()
        .zip(&valid_masks)
        .filter_map(|(magnitude, &mask)| (mask != 0).then_some(magnitude.p95))
        .collect();
    let (map_p95, floor) =
        classifier_darkness_floor(&participating_magnitudes, params.darkness_frac);

    let Some(direct) = sections.direct.as_mut() else {
        log_mutable_cost(indirect_mutable_cost);
        log_mutable_cost(animated_direct_mutable_cost);
        return Ok(RuntimeEnvelopeReport {
            dense_reference_map_p95: map_p95,
            darkness_floor: floor,
            indirect_mutable_cost,
            animated_direct_mutable_cost,
            ..Default::default()
        });
    };
    anyhow::ensure!(
        direct.affinity_dims == affinity_dims(base.grid_dimensions)
            && direct.cell_levels.len() == valid_masks.len(),
        "id-41 runtime envelope grid {:?}/{} levels disagrees with base affinity grid {:?}/{} cells",
        direct.affinity_dims,
        direct.cell_levels.len(),
        affinity_dims(base.grid_dimensions),
        valid_masks.len(),
    );

    let original_levels = direct.cell_levels.clone();
    let before_bytes = payload_bytes_for_levels(direct, &valid_masks, &original_levels);
    let before = score_dense_levels(direct, &valid_masks, &original_levels)?;
    let failures_before = before
        .iter()
        .enumerate()
        .filter(|&(cell, score)| {
            valid_masks[cell] != 0 && !passes(*score, &magnitudes[cell], floor, params)
        })
        .count() as u64;

    let mut selected_restore = vec![false; direct.cell_levels.len()];
    let mut smoothing_refined = vec![false; direct.cell_levels.len()];
    loop {
        let scores = score_dense_levels(direct, &valid_masks, &direct.cell_levels)?;
        let mut changed = false;
        for cell in 0..direct.cell_levels.len() {
            if valid_masks[cell] == 0
                || passes(scores[cell], &magnitudes[cell], floor, params)
                || direct.cell_levels[cell] == Level::L0.to_u8()
            {
                continue;
            }
            direct.cell_levels[cell] = Level::L0.to_u8();
            selected_restore[cell] = true;
            changed = true;
        }

        changed |= smooth_to_fixpoint(
            direct,
            &valid_masks,
            &magnitudes,
            floor,
            params,
            &mut smoothing_refined,
        )?;
        if !changed {
            break;
        }
    }

    let final_compacted = compact_direct_valid_probes(direct, base)?;
    let final_scores = score_emitted(direct, &final_compacted)?;
    let failures_after = final_scores
        .iter()
        .enumerate()
        .filter(|&(cell, score)| {
            valid_masks[cell] != 0 && !passes(*score, &magnitudes[cell], floor, params)
        })
        .count() as u64;
    anyhow::ensure!(
        failures_after == 0,
        "id-41 runtime envelope repair left {failures_after} failing participating cell(s)"
    );
    validate_participating_i5(&direct.cell_levels, &valid_masks, direct.affinity_dims)?;

    let after_bytes = final_compacted.delta_subblocks.len() as u64 * size_of::<u16>() as u64;
    let report = RuntimeEnvelopeReport {
        dense_reference_map_p95: map_p95,
        darkness_floor: floor,
        failures_before_repair: failures_before,
        failures_after_repair: failures_after,
        selected_l0_restores: selected_restore.into_iter().filter(|&v| v).count() as u64,
        smoothing_refinements: smoothing_refined.into_iter().filter(|&v| v).count() as u64,
        incremental_payload_bytes: after_bytes.saturating_sub(before_bytes),
        indirect_mutable_cost,
        animated_direct_mutable_cost,
    };
    log::info!(
        "[sh-coarsen] id 41 runtime envelope: dense map p95 {:.6}, floor {:.6}, failures {} -> {}, selected L0 restores {}, smoothing refinements {}, incremental payload {} bytes",
        report.dense_reference_map_p95,
        report.darkness_floor,
        report.failures_before_repair,
        report.failures_after_repair,
        report.selected_l0_restores,
        report.smoothing_refinements,
        report.incremental_payload_bytes,
    );
    log_mutable_cost(report.indirect_mutable_cost);
    log_mutable_cost(report.animated_direct_mutable_cost);
    Ok(report)
}

fn smooth_to_fixpoint(
    direct: &mut DirectShDeltaVolumesSection,
    valid_masks: &[u64],
    magnitudes: &[MagnitudeStats],
    floor: f32,
    params: &CoarsenParams,
    refined: &mut [bool],
) -> anyhow::Result<bool> {
    let [dx, dy, dz] = direct.affinity_dims.map(|v| v as usize);
    let mut any_changed = false;
    loop {
        let mut changed = false;
        for z in 0..dz {
            for y in 0..dy {
                for x in 0..dx {
                    let cell = x + y * dx + z * dx * dy;
                    if valid_masks[cell] == 0 {
                        continue;
                    }
                    if x + 1 < dx {
                        changed |= smooth_pair(
                            direct,
                            valid_masks,
                            magnitudes,
                            floor,
                            params,
                            refined,
                            cell,
                            cell + 1,
                        )?;
                    }
                    if y + 1 < dy {
                        changed |= smooth_pair(
                            direct,
                            valid_masks,
                            magnitudes,
                            floor,
                            params,
                            refined,
                            cell,
                            cell + dx,
                        )?;
                    }
                    if z + 1 < dz {
                        changed |= smooth_pair(
                            direct,
                            valid_masks,
                            magnitudes,
                            floor,
                            params,
                            refined,
                            cell,
                            cell + dx * dy,
                        )?;
                    }
                }
            }
        }
        any_changed |= changed;
        if !changed {
            return Ok(any_changed);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn smooth_pair(
    direct: &mut DirectShDeltaVolumesSection,
    valid_masks: &[u64],
    magnitudes: &[MagnitudeStats],
    floor: f32,
    params: &CoarsenParams,
    refined: &mut [bool],
    a: usize,
    b: usize,
) -> anyhow::Result<bool> {
    if valid_masks[a] == 0 || valid_masks[b] == 0 {
        return Ok(false);
    }
    let (la, lb) = (direct.cell_levels[a], direct.cell_levels[b]);
    if la.abs_diff(lb) < 2 {
        return Ok(false);
    }
    let cell = if la > lb { a } else { b };
    let l1_score = score_dense_cell(direct, valid_masks[cell], cell, Level::L1)?;
    direct.cell_levels[cell] = if passes(l1_score, &magnitudes[cell], floor, params) {
        Level::L1.to_u8()
    } else {
        Level::L0.to_u8()
    };
    refined[cell] = true;
    Ok(true)
}

fn dense_reference_magnitudes(
    base: &OctahedralShVolumeSection,
    base_direct: Option<&DirectShVolumeSection>,
    deltas: DeltaSectionsRef<'_>,
) -> anyhow::Result<Vec<MagnitudeStats>> {
    let dims = base.grid_dimensions;
    let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
    let total_probes = nx
        .checked_mul(ny)
        .and_then(|n| n.checked_mul(nz))
        .ok_or_else(|| anyhow::anyhow!("SH runtime envelope base grid overflow"))?;
    anyhow::ensure!(
        base.probes.len() == total_probes,
        "SH runtime envelope base probe count mismatch"
    );
    let tile_dim = base.tile_dimension as usize;
    let border = base.tile_border as usize;
    anyhow::ensure!(
        tile_dim > border * 2,
        "SH runtime envelope invalid base tile geometry"
    );
    let interior = tile_dim - border * 2;
    let texels = interior * interior;
    let affinity = affinity_dims(dims);
    let (ax, ay, az) = (
        affinity[0] as usize,
        affinity[1] as usize,
        affinity[2] as usize,
    );
    let validity: Vec<u8> = base.probes.iter().map(|probe| probe.validity).collect();
    let inputs = AnalyzeInputs {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dims: dims,
        validity: &validity,
        base_indirect: base,
        base_direct,
        delta_indirect: deltas.indirect,
        delta_direct: deltas.direct,
        delta_anim_direct: deltas.anim_direct,
        protect_aabbs: &[],
        thresholds: &[],
    };
    let indirect = deltas
        .indirect
        .map(DeltaView::from_indirect)
        .filter(|view| view.affinity_dims == affinity);
    let direct = deltas
        .direct
        .map(DeltaView::from_direct)
        .filter(|view| view.affinity_dims == affinity);
    let animated = deltas
        .anim_direct
        .map(DeltaView::from_anim_direct)
        .filter(|view| view.affinity_dims == affinity);
    let mut valid_rank = vec![-1i64; total_probes];
    let mut rank = 0i64;
    for (probe, slot) in valid_rank.iter_mut().enumerate() {
        if validity[probe] != 0 {
            *slot = rank;
            rank += 1;
        }
    }
    let mut output = Vec::with_capacity(ax * ay * az);
    for cz in 0..az {
        for cy in 0..ay {
            for cx in 0..ax {
                let cell = cx + cy * ax + cz * ax * ay;
                let tiles = build_brick_tiles(
                    &inputs,
                    base,
                    tile_dim,
                    interior,
                    border,
                    &valid_rank,
                    dims,
                    cell,
                    cx,
                    cy,
                    cz,
                    ax,
                    ay,
                    &indirect,
                    &direct,
                    &animated,
                );
                output.push(tile_magnitude(&tiles.composed, texels));
            }
        }
    }
    Ok(output)
}

fn base_valid_probe_masks(base: &OctahedralShVolumeSection) -> anyhow::Result<Vec<u64>> {
    anyhow::ensure!(
        base.probes.len() == base.total_probes(),
        "SH runtime envelope base probe count mismatch"
    );
    let affinity = affinity_dims(base.grid_dimensions);
    let mut output = Vec::with_capacity(affinity.iter().map(|&v| v as usize).product());
    for cz in 0..affinity[2] as usize {
        for cy in 0..affinity[1] as usize {
            for cx in 0..affinity[0] as usize {
                let mut mask = 0u64;
                for local in 0..PROBES_PER_CELL {
                    let lx = local % AF;
                    let ly = (local / AF) % AF;
                    let lz = local / (AF * AF);
                    let (px, py, pz) = (cx * AF + lx, cy * AF + ly, cz * AF + lz);
                    if px >= base.grid_dimensions[0] as usize
                        || py >= base.grid_dimensions[1] as usize
                        || pz >= base.grid_dimensions[2] as usize
                    {
                        continue;
                    }
                    let probe = px
                        + py * base.grid_dimensions[0] as usize
                        + pz * base.grid_dimensions[0] as usize * base.grid_dimensions[1] as usize;
                    if base.probes[probe].validity != 0 {
                        mask |= 1u64 << local;
                    }
                }
                output.push(mask);
            }
        }
    }
    Ok(output)
}

fn validate_participating_i5(
    levels: &[u8],
    valid_masks: &[u64],
    dims: [u32; 3],
) -> anyhow::Result<()> {
    let [dx, dy, dz] = dims.map(|v| v as usize);
    anyhow::ensure!(
        levels.len() == dx * dy * dz && valid_masks.len() == levels.len(),
        "id-41 I5 grid shape mismatch"
    );
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let cell = x + y * dx + z * dx * dy;
                for neighbor in [
                    (x + 1 < dx).then_some(cell + 1),
                    (y + 1 < dy).then_some(cell + dx),
                    (z + 1 < dz).then_some(cell + dx * dy),
                ]
                .into_iter()
                .flatten()
                {
                    if valid_masks[cell] != 0 && valid_masks[neighbor] != 0 {
                        anyhow::ensure!(
                            levels[cell].abs_diff(levels[neighbor]) <= 1,
                            "id-41 runtime envelope violated participating I5 between cells {cell} and {neighbor}"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn mutable_indirect_cost(
    section: &DeltaShVolumesSection,
    valid_masks: &[u64],
    mutable: &ScriptMutableDescriptorSlots,
) -> MutableForcedL0Cost {
    let affected_entries = section
        .affinity_lights
        .iter()
        .filter(|&&slot| mutable.indirect_contains(slot))
        .count() as u64;
    mutable_cost(
        27,
        &section.affinity_offsets,
        &section.affinity_lights,
        &section.cell_levels,
        valid_masks,
        section.delta_probe_f16_stride(),
        |slot| mutable.indirect_contains(slot),
        affected_entries,
    )
}

fn mutable_animated_direct_cost(
    section: &AnimatedDirectShDeltaVolumesSection,
    valid_masks: &[u64],
) -> MutableForcedL0Cost {
    // Every id-45 entry has authored animated amplitude and is therefore
    // unbounded today, regardless of whether scripts additionally target it.
    mutable_cost(
        45,
        &section.affinity_offsets,
        &section.affinity_lights,
        &section.cell_levels,
        valid_masks,
        section.delta_probe_f16_stride(),
        |_| true,
        section.affinity_lights.len() as u64,
    )
}

#[allow(clippy::too_many_arguments)]
fn mutable_cost(
    section_id: u32,
    offsets: &[u32],
    lights: &[u32],
    levels: &[u8],
    valid_masks: &[u64],
    probe_stride: usize,
    affected: impl Fn(u32) -> bool,
    affected_entries: u64,
) -> MutableForcedL0Cost {
    let cells = levels.len().min(valid_masks.len());
    let mut forced = levels.to_vec();
    let mut affected_cells = 0u64;
    for (cell, forced_level) in forced.iter_mut().enumerate().take(cells) {
        let start = offsets.get(cell).copied().unwrap_or(0) as usize;
        let end = offsets.get(cell + 1).copied().unwrap_or(start as u32) as usize;
        if lights
            .get(start..end)
            .is_some_and(|entries| entries.iter().copied().any(&affected))
        {
            *forced_level = Level::L0.to_u8();
            affected_cells += 1;
        }
    }
    let bytes = |candidate: &[u8]| payload_bytes(offsets, candidate, valid_masks, probe_stride);
    let current_payload_bytes = bytes(levels);
    let forced_l0_payload_bytes = bytes(&forced);
    let uniform = vec![Level::L0.to_u8(); cells];
    let uniform_l0_payload_bytes = bytes(&uniform);
    MutableForcedL0Cost {
        section_id,
        affected_cells,
        affected_entries,
        current_payload_bytes,
        forced_l0_payload_bytes,
        uniform_l0_payload_bytes,
        forced_l0_retained_ratio: if uniform_l0_payload_bytes == 0 {
            0.0
        } else {
            forced_l0_payload_bytes as f32 / uniform_l0_payload_bytes as f32
        },
    }
}

fn payload_bytes_for_levels(
    direct: &DirectShDeltaVolumesSection,
    valid_masks: &[u64],
    levels: &[u8],
) -> u64 {
    payload_bytes(
        &direct.affinity_offsets,
        levels,
        valid_masks,
        direct.delta_probe_f16_stride(),
    )
}

fn payload_bytes(offsets: &[u32], levels: &[u8], valid_masks: &[u64], probe_stride: usize) -> u64 {
    levels
        .iter()
        .zip(valid_masks)
        .enumerate()
        .map(|(cell, (&level, &validity))| {
            let entries = offsets
                .get(cell + 1)
                .copied()
                .unwrap_or_default()
                .saturating_sub(offsets.get(cell).copied().unwrap_or_default())
                as u64;
            let level = Level::from_u8(level).unwrap_or(Level::L0);
            entries
                * u64::from(kept_mask(level, validity).count_ones())
                * probe_stride as u64
                * size_of::<u16>() as u64
        })
        .sum()
}

fn log_mutable_cost(cost: MutableForcedL0Cost) {
    log::info!(
        "[sh-coarsen] mutable id {} uniform-L0 policy diagnostic: {} affected cell(s), {} affected entry/entries, payload {} -> {} bytes, uniform {} bytes, retained ratio {:.6}; measurement only",
        cost.section_id,
        cost.affected_cells,
        cost.affected_entries,
        cost.current_payload_bytes,
        cost.forced_l0_payload_bytes,
        cost.uniform_l0_payload_bytes,
        cost.forced_l0_retained_ratio,
    );
}

fn affinity_dims(grid: [u32; 3]) -> [u32; 3] {
    grid.map(|dim| dim.div_ceil(AFFINITY_FACTOR))
}

fn bits(mask: u64) -> impl Iterator<Item = usize> {
    let mut remaining = mask;
    std::iter::from_fn(move || {
        if remaining == 0 {
            return None;
        }
        let bit = remaining.trailing_zeros() as usize;
        remaining &= remaining - 1;
        Some(bit)
    })
}

#[cfg(test)]
#[path = "sh_runtime_envelope_tests.rs"]
mod tests;
