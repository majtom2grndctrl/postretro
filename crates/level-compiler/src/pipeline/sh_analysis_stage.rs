//! SH coarsening and `--sh-analyze` at their pipeline seam, and the direct
//! SH delta bake's stats line.
//! See: context/lib/build_pipeline.md §Compiler pipeline

use super::*;

/// Concatenate the CLI (`--sh-protect-aabb`) and mapper-authored
/// (`sh_protect_volume`) protection AABBs into the single list the coarsening
/// classifier reads. Both sources are world-space `[minx,miny,minz,maxx,maxy,maxz]`
/// boxes in engine meters, so the union is a plain concatenation — a brick is
/// protected if it intersects any box from either source.
pub(super) fn combined_protect_aabbs(cli: &[[f32; 6]], map: &[[f32; 6]]) -> Vec<[f32; 6]> {
    let mut combined = Vec::with_capacity(cli.len() + map.len());
    combined.extend_from_slice(cli);
    combined.extend_from_slice(map);
    combined
}

/// Invoke the production classifier only when the map has not selected the
/// uniform-grid fallback. Keeping the predicate outside the closure makes the
/// opt-out a true pre-classification short-circuit.
pub(super) fn run_sh_coarsening_if_enabled(
    enabled: bool,
    classify: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if enabled {
        classify()?;
    }
    Ok(())
}

/// Classify id 41 coarsening levels and stamp them in place before valid-probe
/// compaction consumes the dense payload. Ids 27 and 45 stay uniform L0 because
/// their runtime animation/script amplitudes have no finite bake-known bound.
/// Reads the pre-BC6H base indirect (RGBA16F) for composed magnitude + the sole
/// probe-validity authority. No-op when the base grid is absent/degenerate.
pub(super) fn apply_coarsen_classification(
    args: &Args,
    map_protect_aabbs: &[[f32; 6]],
    base_indirect: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    base_direct: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    delta_sections: &mut delta_sections::PostBakeDeltaSections,
    mutable_descriptors: &crate::delta_drop_policy::ScriptMutableDescriptorSlots,
) -> anyhow::Result<()> {
    // Enforce the owner-approved section scope even on degenerate/absent-base
    // exits. The bake producers currently initialize these arrays to L0, but
    // this boundary owns the policy rather than relying on that detail.
    delta_sections.enforce_id41_only_coarsening_policy();

    let Some(base) = base_indirect else {
        log::warn!("[sh-coarsen] no base SH volume produced; coarsening skipped (uniform L0)");
        return Ok(());
    };
    if base.grid_dimensions == [0, 0, 0] {
        log::warn!("[sh-coarsen] degenerate SH grid; coarsening skipped (uniform L0)");
        return Ok(());
    }
    let validity: Vec<u8> = base.probes.iter().map(|p| p.validity).collect();
    let grid = sh_coarsen::SectionGrid {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dims: base.grid_dimensions,
        validity: &validity,
    };
    let params = sh_coarsen::CoarsenParams::default();

    // Both protection sources reach the classifier: the `--sh-protect-aabb` CLI
    // stand-in and the mapper-authored `sh_protect_volume` brush AABBs. Their
    // union forces every intersecting brick to L0 (dense).
    let protect_aabbs = combined_protect_aabbs(&args.sh_protect_aabbs, map_protect_aabbs);

    // The direct classifier still reads all three dense sections for the
    // composed reference magnitude, but produces levels only for id 41.
    let direct_levels = {
        let all = delta_sections.refs();
        delta_sections.direct.as_ref().map(|_| {
            sh_coarsen::classify_direct_levels(
                base,
                base_direct,
                all,
                grid,
                &protect_aabbs,
                &params,
            )
        })
    };

    // Guard the defensive grid-mismatch path so id 41 is never handed a
    // wrong-length level array that would corrupt compaction / the wire
    // contract. All sections remain uniform L0 in that case.
    let mut direct_classified = false;
    if let (Some(section), Some(levels)) = (delta_sections.direct.as_mut(), direct_levels) {
        let cells = section.affinity_cell_count();
        if levels.len() == cells {
            section.cell_levels = levels;
            direct_classified = true;
        } else {
            section
                .cell_levels
                .fill(postretro_level_format::sh_reconstruct::Level::L0.to_u8());
            log::warn!(
                "[sh-coarsen] direct affinity mismatch (levels {} vs {cells} cells); leaving uniform L0",
                levels.len()
            );
        }
    }
    log::info!(
        "[sh-coarsen] id 41 classification {}; ids 27/45 remain uniform L0",
        if direct_classified {
            "applied"
        } else {
            "not applicable"
        }
    );
    crate::sh_runtime_envelope::apply_runtime_safe_envelope(
        base,
        base_direct,
        delta_sections,
        mutable_descriptors,
        &params,
    )?;
    Ok(())
}

/// Drive the output-preserving SH coarsenability analysis and emit its summary
/// and JSON. Reads captured pre-BC6H base tiles and the three FINALIZED delta
/// sections (post static-light selection + exact-zero-drop, i.e. the emitted
/// set); mutates nothing that reaches the packer.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_sh_analysis(
    args: &Args,
    protect_aabbs: &[[f32; 6]],
    density_params: &sh_coarsen::CoarsenParams,
    base_indirect: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    base_direct: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    density_classification: &sh_density::DensityClassification,
    dense_deltas: Option<sh_analyze::DenseDeltaSections<'_>>,
    delta_indirect: Option<&postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
    delta_direct: Option<
        &postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection,
    >,
    delta_anim_direct: Option<
        &postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    >,
    mover_channel: Option<&postretro_level_format::mover_probe_channel::MoverProbeChannelSection>,
) {
    let Some(base) = base_indirect else {
        log::warn!("[sh-analyze] no base SH volume produced (empty grid?); analysis skipped");
        return;
    };
    if base.grid_dimensions == [0, 0, 0] {
        log::warn!("[sh-analyze] degenerate SH grid; analysis skipped");
        return;
    }
    let validity: Vec<u8> = base.probes.iter().map(|p| p.validity).collect();
    let protect: Vec<sh_analyze::ProtectAabb> = protect_aabbs
        .iter()
        .map(|a| sh_analyze::ProtectAabb {
            min: [a[0], a[1], a[2]],
            max: [a[3], a[4], a[5]],
        })
        .collect();
    let inputs = sh_analyze::AnalyzeInputs {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dims: base.grid_dimensions,
        validity: &validity,
        base_indirect: base,
        base_direct,
        delta_indirect,
        delta_direct,
        delta_anim_direct,
        mover_channel,
        protect_aabbs: &protect,
        thresholds: &sh_analyze::DEFAULT_THRESHOLDS,
    };
    let mut report = sh_analyze::run_analysis(
        &inputs,
        args.sh_density_force_scale
            .unwrap_or(crate::sh_hierarchy::MAX_NODE_SCALE),
        density_params,
    );
    if let Some(dense) = dense_deltas {
        match sh_analyze::run_emitted_reconstruction_analysis(
            &inputs,
            &density_classification.levels,
            &density_classification.scales,
            dense.indirect,
            dense.direct,
            dense.animated_direct,
            delta_indirect,
            delta_direct,
            delta_anim_direct,
            density_params,
        ) {
            Ok(emitted) => report.emitted_reconstruction = Some(emitted),
            Err(error) => {
                log::warn!("[sh-analyze] emitted reconstruction validation skipped: {error}")
            }
        }
    }
    sh_analyze::log_summary(&report);

    let out = args.sh_analyze_out.clone().unwrap_or_else(|| {
        let mut p = args.output.clone().into_os_string();
        p.push(".sh-analysis.json");
        std::path::PathBuf::from(p)
    });
    match sh_analyze::write_json(&report, &out) {
        Ok(()) => log::info!("[sh-analyze] wrote analysis JSON to {}", out.display()),
        Err(e) => log::warn!("[sh-analyze] failed to write {}: {e}", out.display()),
    }
}

pub(super) fn log_direct_sh_delta_stats(
    stats: Option<&direct_sh_bake::DirectDeltaBakeStats>,
    verbose: bool,
) {
    if let Some(stats) = stats {
        let top = stats
            .rows
            .first()
            .expect("a usable direct SH delta section has at least one CSR entry");
        log::info!(
            "DirectShDeltaVolumes: {} delta bytes; top static_index {} (selection slot {}, {} CSR entries, {} bytes)",
            stats.total_bytes,
            top.static_index,
            top.selection_slot,
            top.csr_entry_count,
            top.byte_total,
        );
        if verbose {
            for row in &stats.rows {
                log::info!(
                    "DirectShDeltaVolumes histogram: selection slot {}, static_index {}, {} CSR entries, {} bytes",
                    row.selection_slot,
                    row.static_index,
                    row.csr_entry_count,
                    row.byte_total,
                );
            }
        }
    } else if verbose {
        log::info!("DirectShDeltaVolumes: skipped (no usable selected light deltas)");
    }
}

#[cfg(test)]
pub(crate) fn log_direct_sh_delta_stats_for_test(
    stats: Option<&direct_sh_bake::DirectDeltaBakeStats>,
    verbose: bool,
) {
    log_direct_sh_delta_stats(stats, verbose);
}
