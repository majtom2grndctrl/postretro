// Stored-set packaging for the base indirect and direct SH volumes.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::octahedral::{
    IrradianceAtlasArrayLayout, irradiance_array_tile_location, irradiance_atlas_array_layout,
};
use postretro_level_format::sh_reconstruct::{
    Level, StoredTile, Tile, corner_locals, local_xyz, reconstruct_l2_tile, stored_node_prefix_sum,
    stored_tile_set,
};
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};

use crate::sh_analyze::{
    AnalyzeInputs, DeltaView, LevelKind, brick_world_aabb, build_brick_tiles,
    classifier_darkness_floor, evaluate_hierarchy_node, level_errors,
    level_errors_with_l1_zero_fallback, tile_magnitude,
};
use crate::sh_bake::MAX_SH_ATLAS_DIMENSION;
use crate::sh_coarsen::{
    BrickClass, CoarsenParams, DeltaSectionsRef, classify_levels, classify_levels_with_ceiling,
};

/// Default multiplier for the inherited composed-error relative gates.
pub(crate) const DEFAULT_SH_DENSITY_FIDELITY: f32 = 1.0;

type PackedTile = Vec<OctahedralAtlasTexel>;

/// Final base-density selection plus the bake-summary attribution needed to
/// distinguish classifier choice from storage constraints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DensityClassification {
    pub levels: Vec<Level>,
    pub scales: Vec<u8>,
    /// Bricks whose unconstrained candidate exceeded a present delta's level,
    /// in id 27 / id 41 / id 45 order. A brick can deliberately appear in more
    /// than one bucket when multiple sections constrain it.
    pub delta_pins: [u64; 3],
    /// Bricks the mapper/CLI protection union lowered from a non-L0 candidate.
    pub protection_pins: u64,
    pub hierarchy_blocks: crate::sh_hierarchy::BlockCounts,
}

pub(crate) struct ForcedScaleClassification {
    pub scales: Vec<u8>,
    pub blocks: crate::sh_hierarchy::BlockCounts,
}

/// Build an explicit fixed-level array for the measurement override. Production
/// selection uses [`classify_base_levels`]; this path deliberately retains only
/// the representability checks that an override cannot bypass.
pub(crate) fn force_levels(
    grid_dimensions: [u32; 3],
    probe_validity: &[bool],
    forced_level: Option<Level>,
) -> Result<Vec<Level>, String> {
    let probe_count = checked_probe_count(grid_dimensions)?;
    if probe_validity.len() != probe_count {
        return Err(format!(
            "SH density force level received {} validity entries for grid {grid_dimensions:?}, expected {probe_count}",
            probe_validity.len()
        ));
    }

    let affinity_dimensions = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let mut levels = uniform_l0_levels(grid_dimensions)?;
    let Some(forced_level) = forced_level else {
        return Ok(levels);
    };

    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let brick = brick_index(brick_x, brick_y, brick_z, affinity_dimensions);
                if brick_is_partial(brick_x, brick_y, brick_z, grid_dimensions) {
                    continue;
                }
                if !brick_has_any_valid(brick_x, brick_y, brick_z, grid_dimensions, probe_validity)
                {
                    continue;
                }
                if forced_level == Level::L1
                    && !brick_has_valid_corner(
                        brick_x,
                        brick_y,
                        brick_z,
                        grid_dimensions,
                        probe_validity,
                    )
                {
                    continue;
                }
                levels[brick] = forced_level;
            }
        }
    }
    Ok(levels)
}

/// Uniform L0 storage used for the map-level opt-out and degenerate fallback.
/// It is deliberately a level-array constructor, rather than a special packer
/// branch, so metadata stamps and stored-set payloads still share one path.
pub(crate) fn uniform_l0_levels(grid_dimensions: [u32; 3]) -> Result<Vec<Level>, String> {
    let affinity_dimensions = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    Ok(vec![Level::L0; checked_probe_count(affinity_dimensions)?])
}

/// Compute the most-coarse representable base level for every brick. A base
/// stored slot must never be coarser than any present grid-matched delta entry:
/// id 27/41/45 deltas remain independent valid-only CSR payloads in kept-rank
/// order. L1 keeps valid corners only; L0 can include valid interior probes, and
/// L2's representative can be any valid probe. Compose reconstructs each and
/// writes/adds it at the corresponding base stored slot.
/// Edge bricks are always L0 because neither sparse stored set represents a
/// partial 4×4×4 lattice.
pub(crate) fn storage_level_ceilings(
    grid_dimensions: [u32; 3],
    deltas: DeltaSectionsRef<'_>,
) -> Result<Vec<Level>, String> {
    let affinity_dimensions = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let cells = checked_probe_count(affinity_dimensions)?;
    let mut ceilings = vec![Level::L2; cells];

    apply_delta_ceiling(
        &mut ceilings,
        affinity_dimensions,
        deltas.indirect.map(|section| {
            (
                section.affinity_dims,
                section.affinity_offsets.as_slice(),
                section.cell_levels.as_slice(),
            )
        }),
        "id 27",
    )?;
    apply_delta_ceiling(
        &mut ceilings,
        affinity_dimensions,
        deltas.direct.map(|section| {
            (
                section.affinity_dims,
                section.affinity_offsets.as_slice(),
                section.cell_levels.as_slice(),
            )
        }),
        "id 41",
    )?;
    apply_delta_ceiling(
        &mut ceilings,
        affinity_dimensions,
        deltas.anim_direct.map(|section| {
            (
                section.affinity_dims,
                section.affinity_offsets.as_slice(),
                section.cell_levels.as_slice(),
            )
        }),
        "id 45",
    )?;

    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                if brick_is_partial(brick_x, brick_y, brick_z, grid_dimensions) {
                    ceilings[brick_index(brick_x, brick_y, brick_z, affinity_dimensions)] =
                        Level::L0;
                }
            }
        }
    }
    Ok(ceilings)
}

/// Apply a previously derived storage ceiling to any level source, including
/// the compiler-only force-level bypass. The final packer receives only this
/// already-clamped array, keeping metadata stamps and stored payload membership
/// in lockstep.
pub(crate) fn apply_storage_level_ceilings(levels: &mut [Level], ceilings: &[Level]) {
    debug_assert_eq!(levels.len(), ceilings.len());
    for (level, ceiling) in levels.iter_mut().zip(ceilings) {
        if level.to_u8() > ceiling.to_u8() {
            *level = *ceiling;
        }
    }
}

/// Apply the non-error-gate constraints to the force-level measurement path.
/// A force level intentionally bypasses classification, but it must not bypass
/// delta ceilings, mapper protection, or the post-constraint seam invariant.
pub(crate) fn apply_forced_level_constraints(
    levels: &mut [Level],
    base: &OctahedralShVolumeSection,
    ceilings: &[Level],
    protect_aabbs: &[[f32; 6]],
) -> Result<(), String> {
    let dimensions = base.grid_dimensions;
    let affinity_dimensions = dimensions.map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? || levels.len() != ceilings.len() {
        return Err("base SH forced-level constraints do not match the affinity grid".to_string());
    }
    let validity: Vec<bool> = base
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    if validity.len() != checked_probe_count(dimensions)? {
        return Err("base SH forced-level validity does not match the probe grid".to_string());
    }
    apply_storage_level_ceilings(levels, ceilings);

    let (ax, ay, az) = (
        affinity_dimensions[0] as usize,
        affinity_dimensions[1] as usize,
        affinity_dimensions[2] as usize,
    );
    let mut participating = vec![false; levels.len()];
    let mut l1_eligible = vec![false; levels.len()];
    for z in 0..az {
        for y in 0..ay {
            for x in 0..ax {
                let brick = brick_index(x, y, z, affinity_dimensions);
                participating[brick] = brick_has_any_valid(x, y, z, dimensions, &validity);
                l1_eligible[brick] = brick_has_valid_corner(x, y, z, dimensions, &validity);
                if brick_intersects_protection(
                    x,
                    y,
                    z,
                    dimensions,
                    base.grid_origin,
                    base.cell_size,
                    protect_aabbs,
                ) {
                    levels[brick] = Level::L0;
                }
            }
        }
    }
    loop {
        let mut changed = false;
        for z in 0..az {
            for y in 0..ay {
                for x in 0..ax {
                    let brick = brick_index(x, y, z, affinity_dimensions);
                    if !participating[brick] {
                        continue;
                    }
                    if x + 1 < ax {
                        changed |= smooth_forced_pair(
                            levels,
                            &participating,
                            &l1_eligible,
                            brick,
                            brick + 1,
                        );
                    }
                    if y + 1 < ay {
                        changed |= smooth_forced_pair(
                            levels,
                            &participating,
                            &l1_eligible,
                            brick,
                            brick + ax,
                        );
                    }
                    if z + 1 < az {
                        changed |= smooth_forced_pair(
                            levels,
                            &participating,
                            &l1_eligible,
                            brick,
                            brick + ax * ay,
                        );
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

/// Apply the compiler debug override for hierarchy scale while retaining every
/// representability blocker. Only the composed-error gate is bypassed.
pub(crate) fn apply_forced_scale_constraints(
    levels: &mut [Level],
    base: &OctahedralShVolumeSection,
    deltas: DeltaSectionsRef<'_>,
    protect_aabbs: &[[f32; 6]],
    forced_scale: u8,
) -> Result<ForcedScaleClassification, String> {
    let dimensions = base.grid_dimensions;
    let affinity_dimensions = dimensions.map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? {
        return Err("base SH forced-scale levels do not match the affinity grid".to_string());
    }
    let validity: Vec<bool> = base
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let has_delta_entry = |cell: usize| {
        [
            deltas
                .indirect
                .map(|section| section.affinity_offsets.as_slice()),
            deltas
                .direct
                .map(|section| section.affinity_offsets.as_slice()),
            deltas
                .anim_direct
                .map(|section| section.affinity_offsets.as_slice()),
        ]
        .into_iter()
        .flatten()
        .any(|offsets| {
            offsets
                .get(cell..=cell + 1)
                .is_some_and(|pair| pair[0] != pair[1])
        })
    };
    let mut bricks = Vec::with_capacity(levels.len());
    for z in 0..affinity_dimensions[2] as usize {
        for y in 0..affinity_dimensions[1] as usize {
            for x in 0..affinity_dimensions[0] as usize {
                let cell = brick_index(x, y, z, affinity_dimensions);
                let mut mask = [false; 64];
                for (local, valid) in mask.iter_mut().enumerate() {
                    let (lx, ly, lz) = local_xyz(local);
                    let (px, py, pz) = (x * 4 + lx, y * 4 + ly, z * 4 + lz);
                    if px < dimensions[0] as usize
                        && py < dimensions[1] as usize
                        && pz < dimensions[2] as usize
                    {
                        *valid = validity[probe_index(px, py, pz, dimensions)];
                    }
                }
                bricks.push(crate::sh_hierarchy::BrickInput {
                    level: levels[cell],
                    participates: mask.iter().any(|&valid| valid),
                    partial: brick_is_partial(x, y, z, dimensions),
                    protected: brick_intersects_protection(
                        x,
                        y,
                        z,
                        dimensions,
                        base.grid_origin,
                        base.cell_size,
                        protect_aabbs,
                    ),
                    has_delta_entry: has_delta_entry(cell),
                    stored_tiles: [
                        postretro_level_format::sh_reconstruct::stored_tiles(Level::L0, &mask)
                            as u32,
                        postretro_level_format::sh_reconstruct::stored_tiles(Level::L1, &mask)
                            as u32,
                        postretro_level_format::sh_reconstruct::stored_tiles(Level::L2, &mask)
                            as u32,
                    ],
                });
            }
        }
    }
    let projection =
        crate::sh_hierarchy::project(affinity_dimensions, &bricks, forced_scale, |_, _, level| {
            crate::sh_hierarchy::NodeEvaluation {
                passes: level != Level::L0,
                ..Default::default()
            }
        })?;
    for (level, assignment) in levels.iter_mut().zip(&projection.assignments) {
        *level = assignment.level;
    }
    Ok(ForcedScaleClassification {
        scales: projection
            .assignments
            .iter()
            .map(|assignment| assignment.scale)
            .collect(),
        blocks: projection.blocks,
    })
}

fn brick_intersects_protection(
    brick_x: usize,
    brick_y: usize,
    brick_z: usize,
    dimensions: [u32; 3],
    grid_origin: [f32; 3],
    cell_size: [f32; 3],
    protect_aabbs: &[[f32; 6]],
) -> bool {
    let max = [
        ((brick_x + 1) * 4 - 1).min(dimensions[0] as usize - 1),
        ((brick_y + 1) * 4 - 1).min(dimensions[1] as usize - 1),
        ((brick_z + 1) * 4 - 1).min(dimensions[2] as usize - 1),
    ];
    let min = [brick_x * 4, brick_y * 4, brick_z * 4];
    protect_aabbs.iter().any(|aabb| {
        (0..3).all(|axis| {
            grid_origin[axis] + min[axis] as f32 * cell_size[axis] <= aabb[axis + 3]
                && grid_origin[axis] + max[axis] as f32 * cell_size[axis] >= aabb[axis]
        })
    })
}

fn smooth_forced_pair(
    levels: &mut [Level],
    participating: &[bool],
    l1_eligible: &[bool],
    a: usize,
    b: usize,
) -> bool {
    if !participating[a] || !participating[b] {
        return false;
    }
    let (a_level, b_level) = (levels[a].to_u8(), levels[b].to_u8());
    if a_level.abs_diff(b_level) < 2 {
        return false;
    }
    let coarse = if a_level > b_level { a } else { b };
    levels[coarse] = match levels[coarse] {
        Level::L2 if l1_eligible[coarse] => Level::L1,
        Level::L2 | Level::L1 | Level::L0 => Level::L0,
    };
    true
}

/// Build the base-SH classifier inputs from the dense composed receiver field
/// and apply mapper protection plus every present delta ceiling before the
/// shared smoothing fixpoint. This must run before delta valid-probe
/// compaction: [`DeltaView`] strides payloads by their dense validity masks.
pub(crate) fn classify_base_levels(
    base_indirect: &OctahedralShVolumeSection,
    base_direct: Option<&DirectShVolumeSection>,
    deltas: DeltaSectionsRef<'_>,
    protect_aabbs: &[[f32; 6]],
    params: &CoarsenParams,
) -> Result<DensityClassification, String> {
    let dimensions = base_indirect.grid_dimensions;
    let affinity_dimensions = dimensions.map(|dimension| dimension.div_ceil(4));
    let brick_count = checked_probe_count(affinity_dimensions)?;
    let (nx, ny, nz) = (
        dimensions[0] as usize,
        dimensions[1] as usize,
        dimensions[2] as usize,
    );
    let total_probes = nx
        .checked_mul(ny)
        .and_then(|count| count.checked_mul(nz))
        .ok_or_else(|| "SH density grid dimensions overflow usize".to_string())?;
    let tile_dimension = base_indirect.tile_dimension as usize;
    let border = base_indirect.tile_border as usize;
    let interior = tile_dimension.saturating_sub(2 * border);
    if total_probes == 0 || interior == 0 || brick_count == 0 {
        return Ok(DensityClassification {
            levels: vec![Level::L0; brick_count],
            scales: vec![0; brick_count],
            ..Default::default()
        });
    }

    let validity: Vec<u8> = base_indirect
        .probes
        .iter()
        .map(|probe| probe.validity)
        .collect();
    if validity.len() != total_probes {
        return Err(format!(
            "SH density base metadata has {} probes for grid {dimensions:?}, expected {total_probes}",
            validity.len()
        ));
    }
    let mut valid_rank = vec![-1i64; total_probes];
    let mut rank = 0i64;
    for (probe, rank_slot) in valid_rank.iter_mut().enumerate() {
        if validity[probe] != 0 {
            *rank_slot = rank;
            rank += 1;
        }
    }

    let delta_indirect = deltas
        .indirect
        .map(DeltaView::from_indirect)
        .filter(|section| section.affinity_dims == affinity_dimensions);
    let delta_direct = deltas
        .direct
        .map(DeltaView::from_direct)
        .filter(|section| section.affinity_dims == affinity_dimensions);
    let delta_animated = deltas
        .anim_direct
        .map(DeltaView::from_anim_direct)
        .filter(|section| section.affinity_dims == affinity_dimensions);
    let inputs = AnalyzeInputs {
        grid_origin: base_indirect.grid_origin,
        cell_size: base_indirect.cell_size,
        grid_dims: dimensions,
        validity: &validity,
        base_indirect,
        base_direct,
        delta_indirect: deltas.indirect,
        delta_direct: deltas.direct,
        delta_anim_direct: deltas.anim_direct,
        protect_aabbs: &[],
        thresholds: &[],
    };
    let texels = interior * interior;
    let weights = vec![1.0; texels];
    let (ax, ay, az) = (
        affinity_dimensions[0] as usize,
        affinity_dimensions[1] as usize,
        affinity_dimensions[2] as usize,
    );
    let mut bricks = Vec::with_capacity(brick_count);
    let mut hierarchy_truth: Vec<Option<Tile>> = vec![None; total_probes];
    let mut valid_masks = Vec::with_capacity(brick_count);
    let mut brick_in_bounds = Vec::with_capacity(brick_count);
    let mut brick_protected = Vec::with_capacity(brick_count);
    for cell_z in 0..az {
        for cell_y in 0..ay {
            for cell_x in 0..ax {
                let cell = cell_x + cell_y * ax + cell_z * ax * ay;
                let tiles = build_brick_tiles(
                    &inputs,
                    base_indirect,
                    tile_dimension,
                    interior,
                    border,
                    &valid_rank,
                    dimensions,
                    cell,
                    cell_x,
                    cell_y,
                    cell_z,
                    ax,
                    ay,
                    &delta_indirect,
                    &delta_direct,
                    &delta_animated,
                );
                let mut valid_mask = [false; 64];
                let mut in_bounds = 0u32;
                for (local, valid) in valid_mask.iter_mut().enumerate() {
                    let (local_x, local_y, local_z) = local_xyz(local);
                    let probe_x = cell_x * 4 + local_x;
                    let probe_y = cell_y * 4 + local_y;
                    let probe_z = cell_z * 4 + local_z;
                    if probe_x >= nx || probe_y >= ny || probe_z >= nz {
                        continue;
                    }
                    in_bounds += 1;
                    let probe = probe_x + probe_y * nx + probe_z * nx * ny;
                    *valid = validity[probe] != 0;
                    if *valid {
                        hierarchy_truth[probe] = tiles.composed[local].clone();
                    }
                }
                let magnitude = tile_magnitude(&tiles.composed, texels);
                let l1 =
                    level_errors_with_l1_zero_fallback(&tiles.composed, texels, interior, &weights);
                let l2 = level_errors(&tiles.composed, LevelKind::L2, texels, interior, &weights);
                // The zero fallback faithfully scores what reconstruction would
                // produce for a missing L1 corner lattice, but a v11 base
                // brick with no valid corner is not allowed to carry an L1
                // stamp. Reserve L1 for bricks with an actual valid corner;
                // otherwise the classifier can still use its always-
                // representable L2 mean or L0.
                let local_is_valid = |local: usize| {
                    let (local_x, local_y, local_z) = local_xyz(local);
                    let probe_x = cell_x * 4 + local_x;
                    let probe_y = cell_y * 4 + local_y;
                    let probe_z = cell_z * 4 + local_z;
                    probe_x < nx
                        && probe_y < ny
                        && probe_z < nz
                        && validity[probe_x + probe_y * nx + probe_z * nx * ny] != 0
                };
                let l1_has_stored_corner = corner_locals().into_iter().any(local_is_valid);
                let (world_min, world_max) =
                    brick_world_aabb(&inputs, dimensions, cell_x, cell_y, cell_z);
                valid_masks.push(valid_mask);
                brick_in_bounds.push(in_bounds);
                brick_protected.push(protect_aabbs.iter().any(|aabb| {
                    (0..3).all(|axis| {
                        world_min[axis] <= aabb[axis + 3] && world_max[axis] >= aabb[axis]
                    })
                }));
                bricks.push(BrickClass {
                    mag_p95: magnitude.p95,
                    mag_max: magnitude.max,
                    l1_p95: l1.p95,
                    l1_max: l1.max,
                    l1_evaluable: l1.texel_samples > 0 && l1_has_stored_corner,
                    l2_p95: l2.p95,
                    l2_max: l2.max,
                    l2_evaluable: l2.texel_samples > 0,
                    has_any_valid: (0..64).any(local_is_valid),
                    world_min: world_min.to_array(),
                    world_max: world_max.to_array(),
                });
            }
        }
    }

    let candidate = classify_levels(&bricks, affinity_dimensions, &[], params);
    let candidate_levels: Vec<Level> = candidate
        .iter()
        .map(|&level| Level::from_u8(level).expect("classifier only produces supported levels"))
        .collect();
    let ceilings = storage_level_ceilings(dimensions, deltas)?;
    let delta_pins = delta_pin_counts(&candidate_levels, dimensions, deltas)?;
    let protection_pins = protection_pin_count(&candidate_levels, base_indirect, protect_aabbs)?;
    let ceiling_bytes: Vec<u8> = ceilings.iter().map(|level| level.to_u8()).collect();
    let levels: Vec<Level> = classify_levels_with_ceiling(
        &bricks,
        affinity_dimensions,
        protect_aabbs,
        &ceiling_bytes,
        params,
    )
    .into_iter()
    .map(|level| Level::from_u8(level).expect("classifier only produces supported levels"))
    .collect();
    let has_delta_entry = |cell: usize| {
        [&delta_indirect, &delta_direct, &delta_animated]
            .into_iter()
            .flatten()
            .any(|view| {
                view.offsets
                    .get(cell..=cell + 1)
                    .is_some_and(|offsets| offsets[0] != offsets[1])
            })
    };
    let hierarchy_bricks: Vec<crate::sh_hierarchy::BrickInput> = levels
        .iter()
        .copied()
        .enumerate()
        .map(|(cell, level)| crate::sh_hierarchy::BrickInput {
            level,
            participates: bricks[cell].has_any_valid,
            partial: brick_in_bounds[cell] != 64,
            protected: brick_protected[cell],
            has_delta_entry: has_delta_entry(cell),
            stored_tiles: [
                postretro_level_format::sh_reconstruct::stored_tiles(Level::L0, &valid_masks[cell])
                    as u32,
                postretro_level_format::sh_reconstruct::stored_tiles(Level::L1, &valid_masks[cell])
                    as u32,
                postretro_level_format::sh_reconstruct::stored_tiles(Level::L2, &valid_masks[cell])
                    as u32,
            ],
        })
        .collect();
    let magnitudes: Vec<f32> = bricks
        .iter()
        .filter(|brick| brick.has_any_valid)
        .map(|brick| brick.mag_p95)
        .collect();
    let (_, darkness_floor) = classifier_darkness_floor(&magnitudes, params.darkness_frac);
    // The scale study selected one production hierarchy level. Larger scales
    // remain wire-supported and are exercised only through the force flag.
    let projection = crate::sh_hierarchy::project(
        affinity_dimensions,
        &hierarchy_bricks,
        1,
        |origin, scale, level| {
            evaluate_hierarchy_node(
                &hierarchy_truth,
                dimensions,
                origin,
                scale,
                level,
                texels,
                darkness_floor,
                params,
            )
        },
    )?;
    let levels = projection
        .assignments
        .iter()
        .map(|assignment| assignment.level)
        .collect();
    let scales = projection
        .assignments
        .iter()
        .map(|assignment| assignment.scale)
        .collect();
    Ok(DensityClassification {
        levels,
        scales,
        delta_pins,
        protection_pins,
        hierarchy_blocks: projection.blocks,
    })
}

/// Count direct ceiling constraints per section for the final bake summary.
/// `levels` must be the source's candidate array before its ceiling is applied;
/// this keeps the attribution meaningful for both the adaptive classifier and
/// the compiler-only forced-level path.
pub(crate) fn delta_pin_counts(
    levels: &[Level],
    grid_dimensions: [u32; 3],
    deltas: DeltaSectionsRef<'_>,
) -> Result<[u64; 3], String> {
    let affinity_dimensions = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? {
        return Err("base SH delta pin attribution does not match the affinity grid".to_string());
    }
    let candidates: Vec<u8> = levels.iter().map(|level| level.to_u8()).collect();
    Ok([
        count_delta_pins(
            &candidates,
            affinity_dimensions,
            deltas.indirect.map(|section| {
                (
                    section.affinity_dims,
                    section.affinity_offsets.as_slice(),
                    section.cell_levels.as_slice(),
                )
            }),
        ),
        count_delta_pins(
            &candidates,
            affinity_dimensions,
            deltas.direct.map(|section| {
                (
                    section.affinity_dims,
                    section.affinity_offsets.as_slice(),
                    section.cell_levels.as_slice(),
                )
            }),
        ),
        count_delta_pins(
            &candidates,
            affinity_dimensions,
            deltas.anim_direct.map(|section| {
                (
                    section.affinity_dims,
                    section.affinity_offsets.as_slice(),
                    section.cell_levels.as_slice(),
                )
            }),
        ),
    ])
}

/// Count mapper/CLI protection demotions for the bake summary before the
/// protection phase changes the level array.
pub(crate) fn protection_pin_count(
    levels: &[Level],
    base: &OctahedralShVolumeSection,
    protect_aabbs: &[[f32; 6]],
) -> Result<u64, String> {
    let dimensions = base.grid_dimensions;
    let affinity_dimensions = dimensions.map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? {
        return Err("base SH protection attribution does not match the affinity grid".to_string());
    }
    let (ax, ay, az) = (
        affinity_dimensions[0] as usize,
        affinity_dimensions[1] as usize,
        affinity_dimensions[2] as usize,
    );
    let mut pins = 0u64;
    for z in 0..az {
        for y in 0..ay {
            for x in 0..ax {
                let brick = brick_index(x, y, z, affinity_dimensions);
                if levels[brick] != Level::L0
                    && brick_intersects_protection(
                        x,
                        y,
                        z,
                        dimensions,
                        base.grid_origin,
                        base.cell_size,
                        protect_aabbs,
                    )
                {
                    pins += 1;
                }
            }
        }
    }
    Ok(pins)
}

fn apply_delta_ceiling(
    ceilings: &mut [Level],
    expected_dimensions: [u32; 3],
    section: Option<([u32; 3], &[u32], &[u8])>,
    label: &str,
) -> Result<(), String> {
    let Some((dimensions, offsets, levels)) = section else {
        return Ok(());
    };
    if dimensions != expected_dimensions {
        return Ok(());
    }
    if offsets.len() != ceilings.len() + 1 || levels.len() != ceilings.len() {
        return Err(format!(
            "{label} delta metadata does not match base affinity grid {expected_dimensions:?}"
        ));
    }
    for cell in 0..ceilings.len() {
        if offsets[cell + 1] <= offsets[cell] {
            continue;
        }
        let level = Level::from_u8(levels[cell]).ok_or_else(|| {
            format!(
                "{label} delta has invalid cell level {} at cell {cell}",
                levels[cell]
            )
        })?;
        if level.to_u8() < ceilings[cell].to_u8() {
            ceilings[cell] = level;
        }
    }
    Ok(())
}

fn count_delta_pins(
    candidates: &[u8],
    expected_dimensions: [u32; 3],
    section: Option<([u32; 3], &[u32], &[u8])>,
) -> u64 {
    let Some((dimensions, offsets, levels)) = section else {
        return 0;
    };
    if dimensions != expected_dimensions
        || offsets.len() != candidates.len() + 1
        || levels.len() != candidates.len()
    {
        return 0;
    }
    candidates
        .iter()
        .enumerate()
        .filter(|&(cell, candidate)| offsets[cell + 1] > offsets[cell] && *candidate > levels[cell])
        .count() as u64
}

/// Repack the indirect bake's legacy valid-probe-order lossless intermediate
/// into the v11 node-aware stored set. The grouped bake cache remains upstream:
/// this is deliberately invoked only after cold and warm assembly converge.
#[cfg(test)]
pub(crate) fn pack_indirect_section(
    section: OctahedralShVolumeSection,
    forced_level: Option<Level>,
) -> Result<(OctahedralShVolumeSection, DensityPackStats), String> {
    let validity: Vec<bool> = section
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let levels = force_levels(section.grid_dimensions, &validity, forced_level)?;
    let scales = vec![0; levels.len()];
    pack_indirect_section_with_levels(section, &levels, &scales)
}

/// Repack the dense RGBA16F base bake using a final, already-representable and
/// delta-clamped per-brick level array. This is deliberately separate from the
/// legacy force-level wrapper above: the pipeline selects these levels after all
/// delta classifiers and the runtime-safe envelope have settled.
pub(crate) fn pack_indirect_section_with_levels(
    mut section: OctahedralShVolumeSection,
    levels: &[Level],
    scales: &[u8],
) -> Result<(OctahedralShVolumeSection, DensityPackStats), String> {
    require_rgba16f(section.irradiance_format, "indirect")?;
    let validity: Vec<bool> = section
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let affinity_dimensions = section
        .grid_dimensions
        .map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? || scales.len() != levels.len() {
        return Err(
            "SH density final level/scale count does not match the affinity grid".to_string(),
        );
    }
    let source_tiles = decode_indirect_intermediate_tiles(&section, &validity)?;
    stamp_assignments(&mut section, levels, scales)?;
    let (tiles, prefix) = stored_tiles(
        section.grid_dimensions,
        &validity,
        levels,
        scales,
        &source_tiles,
    )?;
    let layout = stored_layout(prefix.total_stored_tiles, section.tile_dimension)?;
    section.atlas_dimensions = [layout.atlas_width, layout.atlas_height];
    section.layer_count = layout.layer_count;
    section.tiles_per_layer = layout.tiles_per_layer;
    section.atlas_tiles_per_row = layout.atlas_tiles_per_row;
    section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    section.compact_atlas = pack_tiles_into_atlas(&tiles, layout, section.tile_dimension);

    Ok((
        section,
        DensityPackStats::from_assignments(levels, scales, affinity_dimensions, &prefix),
    ))
}

/// Repack direct SH from its dense cacheable intermediate using the exact stored
/// slots and levels already emitted by id 34. Direct has no validity metadata of
/// its own, so id 34 remains the sole membership source.
pub(crate) fn pack_direct_section(
    mut section: DirectShVolumeSection,
    base: &OctahedralShVolumeSection,
) -> Result<DirectShVolumeSection, String> {
    require_rgba16f(section.irradiance_format, "direct")?;
    if section.grid_dimensions != base.grid_dimensions
        || section.tile_dimension != base.tile_dimension
        || section.tile_border != base.tile_border
    {
        return Err(format!(
            "direct SH dense intermediate does not match id 34 grid/tile geometry: direct grid {:?}, tile {}/{}; base grid {:?}, tile {}/{}",
            section.grid_dimensions,
            section.tile_dimension,
            section.tile_border,
            base.grid_dimensions,
            base.tile_dimension,
            base.tile_border,
        ));
    }

    let validity: Vec<bool> = base
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let levels = levels_from_base(base)?;
    let scales = scales_from_base(base)?;
    let source_tiles = decode_dense_tiles(&section, &validity)?;
    let (tiles, prefix) = stored_tiles(
        section.grid_dimensions,
        &validity,
        &levels,
        &scales,
        &source_tiles,
    )?;
    let layout = stored_layout(prefix.total_stored_tiles, section.tile_dimension)?;
    section.atlas_dimensions = [layout.atlas_width, layout.atlas_height];
    section.layer_count = layout.layer_count;
    section.tiles_per_layer = layout.tiles_per_layer;
    section.atlas_tiles_per_row = layout.atlas_tiles_per_row;
    section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    section.atlas = pack_tiles_into_atlas(&tiles, layout, section.tile_dimension);
    Ok(section)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DensityPackStats {
    pub(crate) brick_levels: [u32; 3],
    pub(crate) node_histogram: [[u32; 3]; 4],
    pub(crate) stored_tiles: u32,
}

impl DensityPackStats {
    fn from_assignments(
        levels: &[Level],
        scales: &[u8],
        affinity_dimensions: [u32; 3],
        prefix: &postretro_level_format::sh_reconstruct::StoredBrickPrefixSum,
    ) -> Self {
        let mut brick_levels = [0; 3];
        let mut node_histogram = [[0; 3]; 4];
        for level in levels {
            brick_levels[level.to_u8() as usize] += 1;
        }
        for z in 0..affinity_dimensions[2] as usize {
            for y in 0..affinity_dimensions[1] as usize {
                for x in 0..affinity_dimensions[0] as usize {
                    let brick = brick_index(x, y, z, affinity_dimensions);
                    let scale = scales[brick];
                    let edge = 1usize << scale;
                    if x % edge == 0
                        && y % edge == 0
                        && z % edge == 0
                        && prefix.bricks[brick].stored_tile_count > 0
                    {
                        node_histogram[scale as usize][levels[brick].to_u8() as usize] += 1;
                    }
                }
            }
        }
        Self {
            brick_levels,
            node_histogram,
            stored_tiles: prefix.total_stored_tiles,
        }
    }
}

pub(crate) fn log_density_summary(
    stats: DensityPackStats,
    classification: &DensityClassification,
    indirect_section_bytes: usize,
    direct_section_bytes: Option<usize>,
    irradiance_format: u32,
) {
    log::info!(
        "[Compiler] SH base-density summary: L0/L1/L2 bricks {}/{}/{}, stored tiles {}, delta pins id27/id41/id45 {}/{}/{}, protection pins {}, id34 {} bytes, id35 {}, format tag {}",
        stats.brick_levels[0],
        stats.brick_levels[1],
        stats.brick_levels[2],
        stats.stored_tiles,
        classification.delta_pins[0],
        classification.delta_pins[1],
        classification.delta_pins[2],
        classification.protection_pins,
        indirect_section_bytes,
        direct_section_bytes
            .map(|bytes| format!("{bytes} bytes"))
            .unwrap_or_else(|| "absent".to_owned()),
        irradiance_format,
    );
    for (scale, levels) in stats.node_histogram.iter().enumerate() {
        log::info!(
            "[Compiler] SH base-density nodes scale {scale}: L0/L1/L2 {}/{}/{}",
            levels[0],
            levels[1],
            levels[2]
        );
    }
    log::info!(
        "[Compiler] SH base-density hierarchy pins: delta {}, protection {}, partial {}, shape {}, gate {}",
        classification.hierarchy_blocks.delta,
        classification.hierarchy_blocks.protection,
        classification.hierarchy_blocks.partial,
        classification.hierarchy_blocks.member_shape,
        classification.hierarchy_blocks.gate,
    );
}

fn require_rgba16f(format: u32, label: &str) -> Result<(), String> {
    if format == IRRADIANCE_FORMAT_RGBA16F {
        Ok(())
    } else {
        Err(format!(
            "cannot density-pack {label} SH after its at-rest encoding (format tag {format})"
        ))
    }
}

fn checked_probe_count(dimensions: [u32; 3]) -> Result<usize, String> {
    dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })
        .ok_or_else(|| format!("SH density grid dimensions {dimensions:?} overflow usize"))
}

fn brick_index(x: usize, y: usize, z: usize, affinity_dimensions: [u32; 3]) -> usize {
    x + y * affinity_dimensions[0] as usize
        + z * affinity_dimensions[0] as usize * affinity_dimensions[1] as usize
}

fn brick_is_partial(x: usize, y: usize, z: usize, dimensions: [u32; 3]) -> bool {
    (x + 1) * 4 > dimensions[0] as usize
        || (y + 1) * 4 > dimensions[1] as usize
        || (z + 1) * 4 > dimensions[2] as usize
}

fn probe_index(x: usize, y: usize, z: usize, dimensions: [u32; 3]) -> usize {
    x + y * dimensions[0] as usize + z * dimensions[0] as usize * dimensions[1] as usize
}

fn brick_has_valid_corner(
    brick_x: usize,
    brick_y: usize,
    brick_z: usize,
    dimensions: [u32; 3],
    validity: &[bool],
) -> bool {
    corner_locals().into_iter().any(|local| {
        let local_x = local % 4;
        let local_y = (local / 4) % 4;
        let local_z = local / 16;
        let x = brick_x * 4 + local_x;
        let y = brick_y * 4 + local_y;
        let z = brick_z * 4 + local_z;
        if x >= dimensions[0] as usize || y >= dimensions[1] as usize || z >= dimensions[2] as usize
        {
            return false;
        }
        let index = probe_index(x, y, z, dimensions);
        validity[index]
    })
}

fn brick_has_any_valid(
    brick_x: usize,
    brick_y: usize,
    brick_z: usize,
    dimensions: [u32; 3],
    validity: &[bool],
) -> bool {
    for local_z in 0..4 {
        for local_y in 0..4 {
            for local_x in 0..4 {
                let (x, y, z) = (
                    brick_x * 4 + local_x,
                    brick_y * 4 + local_y,
                    brick_z * 4 + local_z,
                );
                if x < dimensions[0] as usize
                    && y < dimensions[1] as usize
                    && z < dimensions[2] as usize
                    && validity[probe_index(x, y, z, dimensions)]
                {
                    return true;
                }
            }
        }
    }
    false
}

fn stamp_assignments(
    section: &mut OctahedralShVolumeSection,
    levels: &[Level],
    scales: &[u8],
) -> Result<(), String> {
    let affinity_dimensions = section
        .grid_dimensions
        .map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? || scales.len() != levels.len() {
        return Err("SH density level/scale count does not match the affinity grid".to_string());
    }
    for z in 0..section.grid_dimensions[2] as usize {
        for y in 0..section.grid_dimensions[1] as usize {
            for x in 0..section.grid_dimensions[0] as usize {
                let brick = brick_index(x / 4, y / 4, z / 4, affinity_dimensions);
                let probe = &mut section.probes[probe_index(x, y, z, section.grid_dimensions)];
                probe.density_level = levels[brick].to_u8();
                probe.node_scale = scales[brick];
            }
        }
    }
    Ok(())
}

fn scales_from_base(base: &OctahedralShVolumeSection) -> Result<Vec<u8>, String> {
    let affinity_dimensions = base.grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let mut scales = Vec::with_capacity(checked_probe_count(affinity_dimensions)?);
    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let index =
                    probe_index(brick_x * 4, brick_y * 4, brick_z * 4, base.grid_dimensions);
                scales.push(base.probes[index].node_scale);
            }
        }
    }
    Ok(scales)
}

fn levels_from_base(base: &OctahedralShVolumeSection) -> Result<Vec<Level>, String> {
    let affinity_dimensions = base.grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let mut levels = Vec::with_capacity(checked_probe_count(affinity_dimensions)?);
    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let index =
                    probe_index(brick_x * 4, brick_y * 4, brick_z * 4, base.grid_dimensions);
                let level = Level::from_u8(base.probes[index].density_level).ok_or_else(|| {
                    format!(
                        "id 34 base metadata has invalid density level {} in brick {brick_x},{brick_y},{brick_z}",
                        base.probes[index].density_level
                    )
                })?;
                levels.push(level);
            }
        }
    }
    Ok(levels)
}

fn decode_indirect_intermediate_tiles(
    section: &OctahedralShVolumeSection,
    validity: &[bool],
) -> Result<Vec<Option<PackedTile>>, String> {
    let mut tiles = vec![None; section.probes.len()];
    let mut valid_rank = 0usize;
    for (probe, &is_valid) in validity.iter().enumerate() {
        if !is_valid {
            continue;
        }
        tiles[probe] = Some(read_tile(
            &section.compact_atlas,
            section.atlas_dimensions,
            section.layer_count,
            section.tiles_per_layer,
            section.atlas_tiles_per_row,
            section.tile_dimension,
            valid_rank,
        )?);
        valid_rank += 1;
    }
    Ok(tiles)
}

fn decode_dense_tiles(
    section: &DirectShVolumeSection,
    validity: &[bool],
) -> Result<Vec<Option<PackedTile>>, String> {
    let mut tiles = vec![None; validity.len()];
    for (probe, &is_valid) in validity.iter().enumerate() {
        if is_valid {
            tiles[probe] = Some(read_tile(
                &section.atlas,
                section.atlas_dimensions,
                section.layer_count,
                section.tiles_per_layer,
                section.atlas_tiles_per_row,
                section.tile_dimension,
                probe,
            )?);
        }
    }
    Ok(tiles)
}

fn read_tile(
    bytes: &[u8],
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    tiles_per_layer: u32,
    atlas_tiles_per_row: u32,
    tile_dimension: u32,
    slot: usize,
) -> Result<PackedTile, String> {
    let width = atlas_dimensions[0] as usize;
    let height = atlas_dimensions[1] as usize;
    let layer_texels = width
        .checked_mul(height)
        .ok_or_else(|| "SH density atlas dimensions overflow".to_string())?;
    let expected_len = layer_texels
        .checked_mul(layer_count as usize)
        .and_then(|texels| texels.checked_mul(8))
        .ok_or_else(|| "SH density atlas byte length overflows".to_string())?;
    if bytes.len() != expected_len {
        return Err(format!(
            "SH density source atlas has {} bytes, expected {expected_len} from its declared geometry",
            bytes.len()
        ));
    }
    let [layer, tile_x, tile_y] =
        irradiance_array_tile_location(slot, tiles_per_layer, atlas_tiles_per_row);
    if layer >= layer_count {
        return Err(format!(
            "SH density source slot {slot} is outside its atlas"
        ));
    }
    let mut tile = Vec::with_capacity((tile_dimension * tile_dimension) as usize);
    for y in 0..tile_dimension as usize {
        for x in 0..tile_dimension as usize {
            let texel = layer as usize * layer_texels
                + (tile_y as usize * tile_dimension as usize + y) * width
                + tile_x as usize * tile_dimension as usize
                + x;
            let byte = texel * 8;
            tile.push(OctahedralAtlasTexel {
                rgba: [
                    u16::from_le_bytes([bytes[byte], bytes[byte + 1]]),
                    u16::from_le_bytes([bytes[byte + 2], bytes[byte + 3]]),
                    u16::from_le_bytes([bytes[byte + 4], bytes[byte + 5]]),
                    u16::from_le_bytes([bytes[byte + 6], bytes[byte + 7]]),
                ],
            });
        }
    }
    Ok(tile)
}

fn stored_tiles(
    dimensions: [u32; 3],
    validity: &[bool],
    levels: &[Level],
    scales: &[u8],
    source_tiles: &[Option<PackedTile>],
) -> Result<
    (
        Vec<PackedTile>,
        postretro_level_format::sh_reconstruct::StoredBrickPrefixSum,
    ),
    String,
> {
    let prefix = stored_node_prefix_sum(dimensions, levels, scales, validity).ok_or_else(|| {
        "SH density stored-set prefix sum rejected its metadata shape".to_string()
    })?;
    if source_tiles.len() != validity.len() {
        return Err("SH density source tile count does not match validity metadata".to_string());
    }
    let tile_texels = source_tiles
        .iter()
        .flatten()
        .next()
        .map(Vec::len)
        .unwrap_or_else(|| (6 * 6) as usize);
    let mut output = Vec::with_capacity(prefix.total_stored_tiles as usize);
    for brick_z in 0..prefix.affinity_dimensions[2] as usize {
        for brick_y in 0..prefix.affinity_dimensions[1] as usize {
            for brick_x in 0..prefix.affinity_dimensions[0] as usize {
                let brick = brick_index(brick_x, brick_y, brick_z, prefix.affinity_dimensions);
                if scales[brick] > 0 {
                    if prefix.bricks[brick].stored_tile_count == 0 {
                        continue;
                    }
                    let node_edge = 1usize << scales[brick];
                    let probe_origin = [brick_x * 4, brick_y * 4, brick_z * 4];
                    let probe_edge = node_edge * 4;
                    match levels[brick] {
                        Level::L0 => {
                            return Err("SH density cannot pack a scaled L0 node".to_string());
                        }
                        Level::L1 => {
                            for corner in 0..8 {
                                let coord = [
                                    probe_origin[0]
                                        + if corner & 1 == 0 { 0 } else { probe_edge - 1 },
                                    probe_origin[1]
                                        + if corner & 2 == 0 { 0 } else { probe_edge - 1 },
                                    probe_origin[2]
                                        + if corner & 4 == 0 { 0 } else { probe_edge - 1 },
                                ];
                                let probe = probe_index(coord[0], coord[1], coord[2], dimensions);
                                output.push(source_tiles[probe].clone().unwrap_or_else(|| {
                                    vec![OctahedralAtlasTexel::default(); tile_texels]
                                }));
                            }
                        }
                        Level::L2 => output.push(node_mean_tile(
                            probe_origin,
                            probe_edge,
                            dimensions,
                            validity,
                            source_tiles,
                            tile_texels,
                        )?),
                    }
                    continue;
                }
                let (mask, brick_tiles) = brick_tiles(
                    brick_x,
                    brick_y,
                    brick_z,
                    dimensions,
                    validity,
                    source_tiles,
                );
                for stored in stored_tile_set(levels[brick], mask) {
                    match stored {
                        StoredTile::Probe(local) => {
                            output.push(brick_tiles[local].clone().unwrap_or_else(|| {
                                vec![OctahedralAtlasTexel::default(); tile_texels]
                            }));
                        }
                        StoredTile::BrickMean => {
                            output.push(l2_mean_tile(&brick_tiles, tile_texels)?)
                        }
                    }
                }
            }
        }
    }
    if output.len() != prefix.total_stored_tiles as usize {
        return Err("SH density packing disagreed with the stored-set prefix sum".to_string());
    }
    Ok((output, prefix))
}

fn node_mean_tile(
    origin: [usize; 3],
    edge: usize,
    dimensions: [u32; 3],
    validity: &[bool],
    source_tiles: &[Option<PackedTile>],
    tile_texels: usize,
) -> Result<PackedTile, String> {
    let mut sums = vec![[0.0f32; 3]; tile_texels];
    let mut count = 0u32;
    for z in origin[2]..origin[2] + edge {
        for y in origin[1]..origin[1] + edge {
            for x in origin[0]..origin[0] + edge {
                let probe = probe_index(x, y, z, dimensions);
                if !validity[probe] {
                    continue;
                }
                let tile = source_tiles[probe].as_ref().ok_or_else(|| {
                    "valid SH node probe is missing its dense source tile".to_string()
                })?;
                for (sum, texel) in sums.iter_mut().zip(tile) {
                    sum[0] += f16_bits_to_f32(texel.rgba[0]);
                    sum[1] += f16_bits_to_f32(texel.rgba[1]);
                    sum[2] += f16_bits_to_f32(texel.rgba[2]);
                }
                count += 1;
            }
        }
    }
    if count == 0 {
        return Err("L2 stored tile requested for an all-invalid node".to_string());
    }
    Ok(sums
        .into_iter()
        .map(|sum| OctahedralAtlasTexel {
            rgba: [
                f32_to_f16_bits(sum[0] / count as f32),
                f32_to_f16_bits(sum[1] / count as f32),
                f32_to_f16_bits(sum[2] / count as f32),
                f32_to_f16_bits(1.0),
            ],
        })
        .collect())
}

fn brick_tiles(
    brick_x: usize,
    brick_y: usize,
    brick_z: usize,
    dimensions: [u32; 3],
    validity: &[bool],
    source_tiles: &[Option<PackedTile>],
) -> (u64, [Option<PackedTile>; 64]) {
    let mut mask = 0u64;
    let tiles = std::array::from_fn(|local| {
        let local_x = local % 4;
        let local_y = (local / 4) % 4;
        let local_z = local / 16;
        let x = brick_x * 4 + local_x;
        let y = brick_y * 4 + local_y;
        let z = brick_z * 4 + local_z;
        if x >= dimensions[0] as usize || y >= dimensions[1] as usize || z >= dimensions[2] as usize
        {
            return None;
        }
        let probe = probe_index(x, y, z, dimensions);
        if validity[probe] {
            mask |= 1u64 << local;
            source_tiles[probe].clone()
        } else {
            None
        }
    });
    (mask, tiles)
}

fn l2_mean_tile(
    tiles: &[Option<PackedTile>; 64],
    tile_texels: usize,
) -> Result<PackedTile, String> {
    let rgb_tiles = std::array::from_fn(|local| {
        tiles[local].as_ref().map(|tile| {
            tile.iter()
                .map(|texel| {
                    glam::Vec3::new(
                        f16_bits_to_f32(texel.rgba[0]),
                        f16_bits_to_f32(texel.rgba[1]),
                        f16_bits_to_f32(texel.rgba[2]),
                    )
                })
                .collect()
        })
    });
    let mean = reconstruct_l2_tile(&rgb_tiles, tile_texels)
        .ok_or_else(|| "L2 stored tile requested for an all-invalid brick".to_string())?;
    Ok(mean
        .into_iter()
        .map(|rgb| OctahedralAtlasTexel {
            rgba: [
                f32_to_f16_bits(rgb.x),
                f32_to_f16_bits(rgb.y),
                f32_to_f16_bits(rgb.z),
                f32_to_f16_bits(1.0),
            ],
        })
        .collect())
}

fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = (bits >> 10) & 0x1f;
    let mantissa = bits & 0x3ff;
    let value = if exp == 0 {
        mantissa as f32 * 2.0f32.powi(-24)
    } else if exp == 0x1f {
        if mantissa == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + mantissa as f32 / 1024.0) * 2.0f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -value } else { value }
}

fn stored_layout(
    tile_count: u32,
    tile_dimension: u32,
) -> Result<IrradianceAtlasArrayLayout, String> {
    irradiance_atlas_array_layout([tile_count, 1, 1], tile_dimension, MAX_SH_ATLAS_DIMENSION)
        .ok_or_else(|| format!("SH density stored atlas cannot fit {tile_count} tile(s)"))
}

fn pack_tiles_into_atlas(
    tiles: &[PackedTile],
    layout: IrradianceAtlasArrayLayout,
    tile_dimension: u32,
) -> Vec<u8> {
    let width = layout.atlas_width as usize;
    let layer_texels = width * layout.atlas_height as usize;
    let mut atlas =
        vec![OctahedralAtlasTexel::default(); layer_texels * layout.layer_count as usize];
    for (slot, tile) in tiles.iter().enumerate() {
        let [layer, tile_x, tile_y] = irradiance_array_tile_location(
            slot,
            layout.tiles_per_layer,
            layout.atlas_tiles_per_row,
        );
        for y in 0..tile_dimension as usize {
            for x in 0..tile_dimension as usize {
                let destination = layer as usize * layer_texels
                    + (tile_y as usize * tile_dimension as usize + y) * width
                    + tile_x as usize * tile_dimension as usize
                    + x;
                atlas[destination] = tile[y * tile_dimension as usize + x];
            }
        }
    }
    let mut bytes = Vec::with_capacity(atlas.len() * 8);
    for texel in atlas {
        for channel in texel.rgba {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level as LogLevel;
    use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
    use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
    use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, irradiance_atlas_array_layout,
    };
    use postretro_level_format::sh_volume::{OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe};
    use postretro_test_log_capture::LogCapture;

    const TILE_DIMENSION: u32 = 6;

    fn raw_indirect(
        dimensions: [u32; 3],
        valid: impl Fn(usize) -> bool,
    ) -> OctahedralShVolumeSection {
        let total = checked_probe_count(dimensions).unwrap();
        let probes: Vec<_> = (0..total)
            .map(|index| OctahedralShProbe {
                validity: u8::from(valid(index)),
                ..Default::default()
            })
            .collect();
        let valid_count = probes.iter().filter(|probe| probe.validity != 0).count() as u32;
        let layout =
            irradiance_atlas_array_layout([valid_count, 1, 1], TILE_DIMENSION, 8192).unwrap();
        let mut atlas = vec![
            OctahedralAtlasTexel::default();
            layout.layer_count as usize
                * layout.atlas_width as usize
                * layout.atlas_height as usize
        ];
        let mut rank = 0usize;
        for (probe, metadata) in probes.iter().enumerate() {
            if metadata.validity == 0 {
                continue;
            }
            let value = f32_to_f16_bits(probe as f32);
            let [layer, tx, ty] = irradiance_array_tile_location(
                rank,
                layout.tiles_per_layer,
                layout.atlas_tiles_per_row,
            );
            for y in 0..TILE_DIMENSION as usize {
                for x in 0..TILE_DIMENSION as usize {
                    let destination = layer as usize
                        * layout.atlas_width as usize
                        * layout.atlas_height as usize
                        + (ty as usize * TILE_DIMENSION as usize + y) * layout.atlas_width as usize
                        + tx as usize * TILE_DIMENSION as usize
                        + x;
                    atlas[destination] = OctahedralAtlasTexel {
                        rgba: [value, value, value, f32_to_f16_bits(1.0)],
                    };
                }
            }
            rank += 1;
        }
        let mut bytes = Vec::new();
        for texel in atlas {
            for channel in texel.rgba {
                bytes.extend_from_slice(&channel.to_le_bytes());
            }
        }
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: dimensions,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [layout.atlas_width, layout.atlas_height],
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            compact_atlas: bytes,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn slot_value(section: &OctahedralShVolumeSection, slot: usize) -> f32 {
        let [layer, tx, ty] = irradiance_array_tile_location(
            slot,
            section.tiles_per_layer,
            section.atlas_tiles_per_row,
        );
        let texel = layer as usize
            * section.atlas_dimensions[0] as usize
            * section.atlas_dimensions[1] as usize
            + ty as usize * TILE_DIMENSION as usize * section.atlas_dimensions[0] as usize
            + tx as usize * TILE_DIMENSION as usize;
        let byte = texel * 8;
        f16_bits_to_f32(u16::from_le_bytes([
            section.compact_atlas[byte],
            section.compact_atlas[byte + 1],
        ]))
    }

    fn raw_direct_from_indirect(section: &OctahedralShVolumeSection) -> DirectShVolumeSection {
        DirectShVolumeSection {
            grid_origin: section.grid_origin,
            cell_size: section.cell_size,
            grid_dimensions: section.grid_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_dimensions: section.atlas_dimensions,
            layer_count: section.layer_count,
            tiles_per_layer: section.tiles_per_layer,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            irradiance_format: section.irradiance_format,
            atlas: section.compact_atlas.clone(),
        }
    }

    fn direct_slot_value(section: &DirectShVolumeSection, slot: usize) -> f32 {
        let [layer, tx, ty] = irradiance_array_tile_location(
            slot,
            section.tiles_per_layer,
            section.atlas_tiles_per_row,
        );
        let texel = layer as usize
            * section.atlas_dimensions[0] as usize
            * section.atlas_dimensions[1] as usize
            + ty as usize * TILE_DIMENSION as usize * section.atlas_dimensions[0] as usize
            + tx as usize * TILE_DIMENSION as usize;
        let byte = texel * 8;
        f16_bits_to_f32(u16::from_le_bytes([
            section.atlas[byte],
            section.atlas[byte + 1],
        ]))
    }

    fn indirect_delta(levels: &[u8], entries: &[bool]) -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [levels.len() as u32, 1, 1],
            tile_dimension: TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX; levels.len()],
            cell_levels: levels.to_vec(),
            affinity_offsets: entries
                .iter()
                .scan(0u32, |offset, &entry| {
                    let current = *offset;
                    *offset += u32::from(entry);
                    Some(current)
                })
                .chain(std::iter::once(
                    entries.iter().filter(|&&entry| entry).count() as u32,
                ))
                .collect(),
            affinity_lights: vec![0; entries.iter().filter(|&&entry| entry).count()],
            delta_subblocks: Vec::new(),
        }
    }

    fn direct_delta(levels: &[u8], entries: &[bool]) -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [levels.len() as u32, 1, 1],
            tile_dimension: TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![u64::MAX; levels.len()],
            cell_levels: levels.to_vec(),
            affinity_offsets: entries
                .iter()
                .scan(0u32, |offset, &entry| {
                    let current = *offset;
                    *offset += u32::from(entry);
                    Some(current)
                })
                .chain(std::iter::once(
                    entries.iter().filter(|&&entry| entry).count() as u32,
                ))
                .collect(),
            affinity_lights: vec![0; entries.iter().filter(|&&entry| entry).count()],
            delta_subblocks: Vec::new(),
        }
    }

    fn animated_direct_delta(
        levels: &[u8],
        entries: &[bool],
    ) -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [levels.len() as u32, 1, 1],
            tile_dimension: TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX; levels.len()],
            cell_levels: levels.to_vec(),
            affinity_offsets: entries
                .iter()
                .scan(0u32, |offset, &entry| {
                    let current = *offset;
                    *offset += u32::from(entry);
                    Some(current)
                })
                .chain(std::iter::once(
                    entries.iter().filter(|&&entry| entry).count() as u32,
                ))
                .collect(),
            affinity_lights: vec![0; entries.iter().filter(|&&entry| entry).count()],
            delta_subblocks: Vec::new(),
        }
    }

    #[test]
    fn stored_set_packing_matches_each_level_on_a_constructed_brick() {
        for (level, expected_slots) in [(Level::L0, 64), (Level::L1, 8), (Level::L2, 1)] {
            let raw = raw_indirect([4, 4, 4], |_| true);
            let (packed, stats) = pack_indirect_section(raw, Some(level)).unwrap();
            assert_eq!(stats.stored_tiles, expected_slots);
            assert_eq!(
                packed
                    .probes
                    .iter()
                    .map(|probe| probe.density_level)
                    .collect::<Vec<_>>(),
                vec![level.to_u8(); 64]
            );
            for (slot, stored) in stored_tile_set(level, u64::MAX).into_iter().enumerate() {
                let expected = match stored {
                    StoredTile::Probe(local) => local as f32,
                    StoredTile::BrickMean => 31.5,
                };
                assert_eq!(slot_value(&packed, slot), expected);
            }
        }
    }

    #[test]
    fn l0_packing_is_brick_major_across_adjacent_bricks() {
        let raw = raw_indirect([8, 4, 4], |_| true);
        let (packed, stats) = pack_indirect_section(raw, None).unwrap();

        assert_eq!(stats.stored_tiles, 128);
        assert_eq!(slot_value(&packed, 0), 0.0);
        assert_eq!(slot_value(&packed, 63), 123.0);
        assert_eq!(slot_value(&packed, 64), 4.0);
        assert_eq!(slot_value(&packed, 127), 127.0);
    }

    #[test]
    fn scale_zero_node_path_is_byte_identical_to_brick_path() {
        let raw = raw_indirect([8, 4, 4], |probe| probe % 5 != 0);
        let (brick_path, brick_stats) = pack_indirect_section(raw.clone(), None).unwrap();
        let levels = vec![Level::L0; 2];
        let scales = vec![0; 2];
        let (node_path, node_stats) =
            pack_indirect_section_with_levels(raw, &levels, &scales).unwrap();

        assert_eq!(node_path, brick_path);
        assert_eq!(node_stats, brick_stats);
        assert!(node_path.probes.iter().all(|probe| probe.node_scale == 0));
        assert_eq!(
            node_path.try_to_bytes().unwrap(),
            brick_path.try_to_bytes().unwrap(),
            "the v11 scale byte occupies the old zero-reserved byte"
        );
    }

    #[test]
    fn l2_packing_synthesizes_the_mean_over_valid_tiles() {
        let raw = raw_indirect([4, 4, 4], |probe| probe != 0);
        let (packed, _) = pack_indirect_section(raw, Some(Level::L2)).unwrap();
        assert!((slot_value(&packed, 0) - 32.0).abs() < 0.01);
    }

    #[test]
    fn l1_packing_reserves_an_invalid_corner_as_a_zero_tile() {
        let raw = raw_indirect([4, 4, 4], |probe| probe != 0);
        let (packed, stats) = pack_indirect_section(raw, Some(Level::L1)).unwrap();
        assert_eq!(stats.stored_tiles, 8);
        assert_eq!(slot_value(&packed, 0), 0.0);
        assert_eq!(slot_value(&packed, 1), 3.0);
    }

    #[test]
    fn scaled_nodes_pack_one_shared_l1_or_l2_basis() {
        let levels = vec![Level::L1; 8];
        let scales = vec![1; 8];
        let (l1, stats) =
            pack_indirect_section_with_levels(raw_indirect([8, 8, 8], |_| true), &levels, &scales)
                .unwrap();
        assert_eq!(stats.stored_tiles, 8);
        assert_eq!(stats.node_histogram[1], [0, 1, 0]);
        assert_eq!(
            (0..8).map(|slot| slot_value(&l1, slot)).collect::<Vec<_>>(),
            vec![0.0, 7.0, 56.0, 63.0, 448.0, 455.0, 504.0, 511.0]
        );
        assert!(l1.probes.iter().all(|probe| probe.node_scale == 1));

        let levels = vec![Level::L2; 8];
        let (l2, stats) =
            pack_indirect_section_with_levels(raw_indirect([8, 8, 8], |_| true), &levels, &scales)
                .unwrap();
        assert_eq!(stats.stored_tiles, 1);
        assert!((slot_value(&l2, 0) - 255.5).abs() < 0.1);

        let raw_direct = raw_direct_from_indirect(&raw_indirect([8, 8, 8], |_| true));
        let direct = pack_direct_section(raw_direct, &l2).unwrap();
        assert_eq!(direct.atlas_dimensions, l2.atlas_dimensions);
        assert_eq!(direct.layer_count, l2.layer_count);
        assert!((direct_slot_value(&direct, 0) - 255.5).abs() < 0.1);
        let decoded_l2 =
            OctahedralShVolumeSection::from_bytes(&l2.try_to_bytes().unwrap()).unwrap();
        let decoded_direct =
            DirectShVolumeSection::from_bytes(&direct.try_to_bytes().unwrap()).unwrap();
        assert_eq!(decoded_l2, l2);
        assert_eq!(decoded_direct, direct);
    }

    #[test]
    fn forced_scale_three_merges_only_a_full_ceiling_free_node() {
        let base = raw_indirect([32, 32, 32], |_| true);
        let mut levels = vec![Level::L2; 8 * 8 * 8];
        let forced =
            apply_forced_scale_constraints(&mut levels, &base, DeltaSectionsRef::default(), &[], 3)
                .unwrap();
        assert!(forced.scales.iter().all(|&scale| scale == 3));
        assert_eq!(forced.blocks.delta, 0);
        assert_eq!(forced.blocks.partial, 0);
        assert_eq!(forced.blocks.protection, 0);
    }

    #[test]
    fn forced_scale_keeps_delta_protection_and_partial_bricks_at_zero() {
        let base = raw_indirect([21, 8, 8], |_| true);
        let affinity = [6, 2, 2];
        let mut direct = direct_delta(&vec![Level::L2.to_u8(); 24], &vec![false; 24]);
        direct.affinity_dims = affinity;
        direct.affinity_offsets = std::iter::once(0)
            .chain(std::iter::repeat_n(1, 24))
            .collect();
        direct.affinity_lights = vec![0];
        let mut levels = vec![Level::L2; 24];
        let forced = apply_forced_scale_constraints(
            &mut levels,
            &base,
            DeltaSectionsRef {
                direct: Some(&direct),
                ..Default::default()
            },
            &[[8.1, 0.1, 0.1, 8.2, 0.2, 0.2]],
            1,
        )
        .unwrap();
        assert_eq!(forced.scales[0], 0, "id 41 member must remain scale zero");
        assert_eq!(
            forced.scales[2], 0,
            "protected member must remain scale zero"
        );
        assert!(forced.scales.iter().enumerate().all(|(brick, &scale)| {
            let x = brick % affinity[0] as usize;
            x < 4 || scale == 0
        }));
        assert!(forced.blocks.delta > 0);
        assert!(forced.blocks.protection > 0);
        assert!(forced.blocks.partial > 0);
    }

    #[test]
    fn summary_reports_node_histogram_and_scale_zero_pin_attribution() {
        let capture = LogCapture::start();
        let stats = DensityPackStats {
            brick_levels: [5, 6, 7],
            node_histogram: [[1, 2, 3], [0, 4, 5], [0; 3], [0; 3]],
            stored_tiles: 42,
        };
        let classification = DensityClassification {
            delta_pins: [8, 9, 10],
            protection_pins: 11,
            hierarchy_blocks: crate::sh_hierarchy::BlockCounts {
                delta: 12,
                protection: 13,
                partial: 14,
                member_shape: 15,
                gate: 16,
            },
            ..Default::default()
        };
        log_density_summary(stats, &classification, 100, Some(200), 2);

        capture.assert_logged_once(LogLevel::Info, "nodes scale 0: L0/L1/L2 1/2/3");
        capture.assert_logged_once(LogLevel::Info, "nodes scale 1: L0/L1/L2 0/4/5");
        capture.assert_logged_once(
            LogLevel::Info,
            "delta pins id27/id41/id45 8/9/10, protection pins 11",
        );
        capture.assert_logged_once(
            LogLevel::Info,
            "hierarchy pins: delta 12, protection 13, partial 14, shape 15, gate 16",
        );
    }

    #[test]
    fn forced_level_keeps_partial_edge_bricks_at_l0() {
        let validity = vec![true; 5 * 4 * 4];
        let levels = force_levels([5, 4, 4], &validity, Some(Level::L2)).unwrap();
        assert_eq!(levels, vec![Level::L2, Level::L0]);

        // P20: an L1 request may not produce an unloadable L1 stamp when a
        // full brick has valid interior probes but no valid corner.
        let mut no_corner_validity = vec![false; 4 * 4 * 4];
        no_corner_validity[1] = true;
        assert_eq!(
            force_levels([4, 4, 4], &no_corner_validity, Some(Level::L1)).unwrap(),
            vec![Level::L0]
        );
    }

    #[test]
    fn delta_ceilings_follow_present_entries_and_partial_edges() {
        // AC7: an id-27 L0 entry pins the first base brick, an id-41 L1 entry
        // pins the second, and an id-45 L0 entry can independently pin it too.
        let indirect = indirect_delta(&[Level::L0.to_u8(), Level::L2.to_u8()], &[true, false]);
        let direct = direct_delta(&[Level::L2.to_u8(), Level::L1.to_u8()], &[false, true]);
        let animated =
            animated_direct_delta(&[Level::L2.to_u8(), Level::L0.to_u8()], &[false, true]);
        let ceilings = storage_level_ceilings(
            [8, 4, 4],
            DeltaSectionsRef {
                indirect: Some(&indirect),
                direct: Some(&direct),
                anim_direct: Some(&animated),
            },
        )
        .unwrap();
        assert_eq!(ceilings, vec![Level::L0, Level::L0]);

        let no_delta = storage_level_ceilings([8, 4, 4], DeltaSectionsRef::default()).unwrap();
        assert_eq!(no_delta, vec![Level::L2, Level::L2]);
        let partial = storage_level_ceilings([5, 4, 4], DeltaSectionsRef::default()).unwrap();
        assert_eq!(partial, vec![Level::L2, Level::L0]);
    }

    #[test]
    fn optout_uniform_levels_are_all_l0() {
        // The pipeline calls this after worldspawn `_sh_coarsen "0"` short
        // circuits both classifiers, so even a full brick has an L0 stamp.
        assert_eq!(
            uniform_l0_levels([8, 4, 4]).unwrap(),
            vec![Level::L0, Level::L0]
        );
    }

    #[test]
    fn forced_level_respects_protection_and_smooths_the_boundary() {
        let base = raw_indirect([8, 4, 4], |_| true);
        let mut levels = vec![Level::L2, Level::L2];
        apply_forced_level_constraints(
            &mut levels,
            &base,
            &[Level::L2, Level::L2],
            &[[0.1, 0.1, 0.1, 0.2, 0.2, 0.2]],
        )
        .unwrap();
        assert_eq!(levels, vec![Level::L0, Level::L1]);
    }
}
