//! GPU-free world-region to resident affinity-row planning.

use super::*;
use crate::render::ShSampleRegion;

const SAMPLE_REGION_DILATION_CELLS: f32 = 1.1;

struct SampleRegionGrid<'a> {
    origin: [f32; 3],
    cell_size: [f32; 3],
    dimensions: [u32; 3],
    dense_nodes: &'a [Option<StoredNode>],
}

impl ShResidencyState {
    /// Resolve app-owned world bounds against current, post-drain residency.
    /// `fog_draw_all` is the established empty-fog-reach sentinel. The caller
    /// owns `rows`, retaining its allocation across warm frames.
    pub(super) fn resolve_sample_region_rows(
        &self,
        regions: &[ShSampleRegion],
        fog_draw_all: bool,
        rows: &mut Vec<u32>,
    ) -> Result<(), ShResidencyDrainError> {
        resolve_regions(
            self.sample_region_grid(),
            regions,
            fog_draw_all,
            &[
                &self.indirect_resident_rows,
                &self.direct_promotion_resident_rows,
                &self.direct_animated_resident_rows,
            ],
            rows,
        )
    }

    /// Close install, slot-reuse, partial-eviction, and eviction work over the
    /// canonical writer of any scaled node touched by the changed rows.
    pub(super) fn close_residency_rows_over_scaled_writers(
        &self,
        changed_rows: impl IntoIterator<Item = u32>,
        resident_rows: &[u32],
        rows: &mut Vec<u32>,
    ) -> Result<(), ShResidencyDrainError> {
        let grid = self.sample_region_grid();
        rows.clear();
        for row in changed_rows {
            if resident_rows.binary_search(&row).is_ok() {
                rows.push(row);
            }
            if let Some(writer) = scaled_writer_row(&grid, row)?
                && resident_rows.binary_search(&writer).is_ok()
            {
                rows.push(writer);
            }
        }
        rows.sort_unstable();
        rows.dedup();
        Ok(())
    }

    fn sample_region_grid(&self) -> SampleRegionGrid<'_> {
        SampleRegionGrid {
            origin: self.base_metadata.grid_origin,
            cell_size: self.base_metadata.cell_size,
            dimensions: self.grid_dimensions,
            dense_nodes: &self.dense_node,
        }
    }
}

fn resolve_regions(
    grid: SampleRegionGrid<'_>,
    regions: &[ShSampleRegion],
    fog_draw_all: bool,
    resident_row_sets: &[&BTreeSet<u32>],
    rows: &mut Vec<u32>,
) -> Result<(), ShResidencyDrainError> {
    rows.clear();
    if fog_draw_all {
        rows.extend(
            resident_row_sets
                .iter()
                .flat_map(|resident_rows| resident_rows.iter().copied()),
        );
    } else {
        for region in regions {
            append_region_rows(&grid, *region, resident_row_sets, rows)?;
        }
        rows.sort_unstable();
        rows.dedup();
    }

    // Do not recursively close writer rows: each sampled row adds at most one.
    let sampled_len = rows.len();
    for index in 0..sampled_len {
        if let Some(writer) = scaled_writer_row(&grid, rows[index])?
            && resident_row_sets
                .iter()
                .any(|resident_rows| resident_rows.contains(&writer))
        {
            rows.push(writer);
        }
    }
    rows.sort_unstable();
    rows.dedup();
    Ok(())
}

fn append_region_rows(
    grid: &SampleRegionGrid<'_>,
    region: ShSampleRegion,
    resident_row_sets: &[&BTreeSet<u32>],
    rows: &mut Vec<u32>,
) -> Result<(), ShResidencyDrainError> {
    if grid.dimensions.contains(&0)
        || grid
            .cell_size
            .iter()
            .any(|spacing| !spacing.is_finite() || *spacing <= 0.0)
        || !region.min.is_finite()
        || !region.max.is_finite()
    {
        return Ok(());
    }

    let min = region.min.min(region.max);
    let max = region.min.max(region.max);
    let origin = glam::Vec3::from_array(grid.origin);
    let spacing = glam::Vec3::from_array(grid.cell_size);
    let dilation = spacing * SAMPLE_REGION_DILATION_CELLS;
    let normalized_min = (min - dilation - origin) / spacing;
    let normalized_max = (max + dilation - origin) / spacing;
    let grid_max = grid.dimensions.map(|dimension| dimension.saturating_sub(1));
    let first = [
        clamp_probe(normalized_min.x.floor(), grid_max[0]),
        clamp_probe(normalized_min.y.floor(), grid_max[1]),
        clamp_probe(normalized_min.z.floor(), grid_max[2]),
    ];
    // `floor + 1` includes the high trilinear corner on a probe plane.
    let last = [
        clamp_probe(normalized_max.x.floor() + 1.0, grid_max[0]),
        clamp_probe(normalized_max.y.floor() + 1.0, grid_max[1]),
        clamp_probe(normalized_max.z.floor() + 1.0, grid_max[2]),
    ];

    for brick_z in first[2] / 4..=last[2] / 4 {
        for brick_y in first[1] / 4..=last[1] / 4 {
            for brick_x in first[0] / 4..=last[0] / 4 {
                let row = affinity_row([brick_x, brick_y, brick_z], grid.dimensions)?;
                if resident_row_sets
                    .iter()
                    .any(|resident_rows| resident_rows.contains(&row))
                {
                    rows.push(row);
                }
            }
        }
    }
    Ok(())
}

fn clamp_probe(value: f32, maximum: u32) -> u32 {
    value.max(0.0).min(maximum as f32) as u32
}

fn scaled_writer_row(
    grid: &SampleRegionGrid<'_>,
    sampled_row: u32,
) -> Result<Option<u32>, ShResidencyDrainError> {
    let affinity_dims = grid.dimensions.map(|dimension| dimension.div_ceil(4));
    let brick = affinity_coordinates(sampled_row, affinity_dims)?;
    let first_probe = brick.map(|coordinate| coordinate * 4);
    let last_probe = [
        (first_probe[0] + 4).min(grid.dimensions[0]),
        (first_probe[1] + 4).min(grid.dimensions[1]),
        (first_probe[2] + 4).min(grid.dimensions[2]),
    ];
    let mut writer = None;
    for z in first_probe[2]..last_probe[2] {
        for y in first_probe[1]..last_probe[1] {
            for x in first_probe[0]..last_probe[0] {
                let dense = dense_index([x, y, z], grid.dimensions)?;
                let Some(node) = grid.dense_nodes.get(dense as usize).copied().flatten() else {
                    continue;
                };
                if node.scale == 0 {
                    continue;
                }
                let row = affinity_row(node.brick_origin, grid.dimensions)?;
                match writer {
                    None => writer = Some(row),
                    Some(prior) if prior == row => {}
                    Some(_) => {
                        return Err(ShResidencyDrainError::GpuCapacity {
                            reason: "one affinity row resolves to multiple scaled-node writers",
                        });
                    }
                }
            }
        }
    }
    Ok(writer)
}

fn affinity_row(brick: [u32; 3], grid_dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    let dims = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    if brick[0] >= dims[0] || brick[1] >= dims[1] || brick[2] >= dims[2] {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    brick[0]
        .checked_add(
            brick[1]
                .checked_mul(dims[0])
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .and_then(|row| row.checked_add(brick[2].checked_mul(dims[0].checked_mul(dims[1])?)?))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn affinity_coordinates(row: u32, dimensions: [u32; 3]) -> Result<[u32; 3], ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let count = xy
        .checked_mul(dimensions[2])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if xy == 0 || row >= count {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    Ok([
        row % dimensions[0],
        (row / dimensions[0]) % dimensions[1],
        row / xy,
    ])
}

fn dense_index(xyz: [u32; 3], dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    xyz[0]
        .checked_add(
            xyz[1]
                .checked_mul(dimensions[0])
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .and_then(|dense| dense.checked_add(xyz[2].checked_mul(xy)?))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn grid<'a>(nodes: &'a [Option<StoredNode>]) -> SampleRegionGrid<'a> {
        SampleRegionGrid {
            origin: [0.0; 3],
            cell_size: [1.0; 3],
            dimensions: [16, 4, 4],
            dense_nodes: nodes,
        }
    }

    #[test]
    fn region_dilation_matches_sampler_footprint() {
        let nodes = vec![None; 16 * 4 * 4];
        let resident = BTreeSet::from([0, 1, 2, 3]);
        let mut rows = Vec::new();
        resolve_regions(
            grid(&nodes),
            &[ShSampleRegion::new(
                glam::Vec3::new(4.91, 1.0, 1.0),
                glam::Vec3::new(4.91, 1.0, 1.0),
            )],
            false,
            &[&resident],
            &mut rows,
        )
        .unwrap();
        assert!(rows.contains(&0));

        resolve_regions(
            grid(&nodes),
            &[ShSampleRegion::new(
                glam::Vec3::new(5.11, 1.0, 1.0),
                glam::Vec3::new(5.11, 1.0, 1.0),
            )],
            false,
            &[&resident],
            &mut rows,
        )
        .unwrap();
        assert!(!rows.contains(&0));
    }

    #[test]
    fn scaled_node_gate_adds_single_writer_row() {
        let scaled = StoredNode {
            brick_origin: [0, 0, 0],
            scale: 1,
            level: 1,
        };
        let mut nodes = vec![None; 16 * 4 * 4];
        for z in 0..4 {
            for y in 0..4 {
                for x in 4..8 {
                    nodes[(x + y * 16 + z * 64) as usize] = Some(scaled);
                }
            }
        }
        let resident = BTreeSet::from([0, 1]);
        let mut rows = Vec::new();
        resolve_regions(
            grid(&nodes),
            &[ShSampleRegion::new(
                glam::Vec3::new(6.0, 1.0, 1.0),
                glam::Vec3::new(6.0, 1.0, 1.0),
            )],
            false,
            &[&resident],
            &mut rows,
        )
        .unwrap();
        assert_eq!(rows, vec![0, 1]);
    }

    proptest! {
        #[test]
        fn scaled_node_gate_adds_at_most_one_canonical_writer_row(
            sampled_row in 0u32..4,
            writer_row in 0u32..4,
            occupied_offsets in prop::collection::vec(0u32..64, 1..65),
        ) {
            let mut nodes = vec![None; 16 * 4 * 4];
            let sampled_x = sampled_row * 4;
            let scaled = StoredNode {
                brick_origin: [writer_row, 0, 0],
                scale: 1,
                level: 1,
            };
            for offset in occupied_offsets {
                let x = sampled_x + offset % 4;
                let y = (offset / 4) % 4;
                let z = offset / 16;
                nodes[(x + y * 16 + z * 64) as usize] = Some(scaled);
            }

            let writer = scaled_writer_row(&grid(&nodes), sampled_row).unwrap();
            prop_assert_eq!(writer, Some(writer_row));
        }
    }

    #[test]
    fn fog_reachability_sentinel_controls_gate_scope() {
        let nodes = vec![None; 16 * 4 * 4];
        let resident = BTreeSet::from([0, 1, 2, 3]);
        let region = ShSampleRegion::new(glam::Vec3::ZERO, glam::Vec3::ZERO);
        let mut rows = Vec::new();
        resolve_regions(grid(&nodes), &[region], false, &[&resident], &mut rows).unwrap();
        assert_ne!(rows, vec![0, 1, 2, 3]);
        resolve_regions(grid(&nodes), &[region], true, &[&resident], &mut rows).unwrap();
        assert_eq!(rows, vec![0, 1, 2, 3]);
    }

    #[test]
    fn residency_change_closes_over_foreign_writer_row() {
        let scaled = StoredNode {
            brick_origin: [0, 0, 0],
            scale: 1,
            level: 2,
        };
        let mut nodes = vec![None; 16 * 4 * 4];
        nodes[4] = Some(scaled);
        let resident = BTreeSet::from([0]);
        let grid = grid(&nodes);
        assert_eq!(scaled_writer_row(&grid, 1).unwrap(), Some(0));
        assert!(resident.contains(&0));
    }

    #[test]
    fn gate_resolution_observes_post_drain_residency() {
        let nodes = vec![None; 16 * 4 * 4];
        let region = ShSampleRegion::new(
            glam::Vec3::new(5.0, 1.0, 1.0),
            glam::Vec3::new(5.0, 1.0, 1.0),
        );
        let mut rows = Vec::new();
        let before = BTreeSet::from([0]);
        resolve_regions(grid(&nodes), &[region], false, &[&before], &mut rows).unwrap();
        assert_eq!(rows, vec![0]);
        let after = BTreeSet::from([1]);
        resolve_regions(grid(&nodes), &[region], false, &[&after], &mut rows).unwrap();
        assert_eq!(rows, vec![1]);
    }

    // Regression: id-41/id-45-only resident rows were omitted from the shared gate.
    #[test]
    fn direct_only_region_rows_enter_the_gate_and_compose_plan() {
        let nodes = vec![None; 16 * 4 * 4];
        let indirect = BTreeSet::from([0]);
        let static_direct = BTreeSet::from([2]);
        let animated_direct = BTreeSet::from([2, 3]);
        let region = ShSampleRegion::new(
            glam::Vec3::new(9.0, 1.0, 1.0),
            glam::Vec3::new(13.0, 1.0, 1.0),
        );
        let mut gated = Vec::new();
        resolve_regions(
            grid(&nodes),
            &[region],
            false,
            &[&indirect, &static_direct, &animated_direct],
            &mut gated,
        )
        .unwrap();
        assert_eq!(gated, vec![2, 3]);

        let controls = compose_plan::ComposeControlSnapshot {
            light_term_mask: LightTermMask::ALL,
            promotion_override: DirectShDebugOverride::default(),
            animated_override: AnimatedDirectShDebugOverride::default(),
        };
        let indirect_rows = [0];
        let static_rows = [2];
        let animated_rows = [2, 3];
        let mut planner = StreamedComposePlanner::default();
        let initial_static_weights = [0.0];
        let animated_weights = [1.0];
        let initial = compose_plan::ComposePlannerFrame {
            records_compose: false,
            force_full_resident: false,
            gated_rows: &gated,
            indirect_rows: compose_plan::ComposePassRows {
                resident: &indirect_rows,
                contributing: &indirect_rows,
            },
            static_direct_rows: compose_plan::ComposePassRows {
                resident: &static_rows,
                contributing: &static_rows,
            },
            animated_direct_rows: compose_plan::ComposePassRows {
                resident: &animated_rows,
                contributing: &animated_rows,
            },
            indirect_active: false,
            animated_direct_active: false,
            effective_static_weights: &initial_static_weights,
            effective_animated_weights: &animated_weights,
            controls,
        };
        planner.plan_frame(initial);

        let changed_static_weights = [0.5];
        let plan = planner.plan_frame(compose_plan::ComposePlannerFrame {
            records_compose: true,
            force_full_resident: false,
            gated_rows: &gated,
            indirect_rows: compose_plan::ComposePassRows {
                resident: &indirect_rows,
                contributing: &indirect_rows,
            },
            static_direct_rows: compose_plan::ComposePassRows {
                resident: &static_rows,
                contributing: &static_rows,
            },
            animated_direct_rows: compose_plan::ComposePassRows {
                resident: &animated_rows,
                contributing: &animated_rows,
            },
            indirect_active: false,
            animated_direct_active: true,
            effective_static_weights: &changed_static_weights,
            effective_animated_weights: &animated_weights,
            controls,
        });
        assert_eq!(plan.static_direct.rows(), &[2]);
        assert_eq!(plan.animated_direct.rows(), &[2, 3]);
    }

    #[test]
    fn compose_planning_reuses_warmed_scratch() {
        let nodes = vec![None; 16 * 4 * 4];
        let resident = BTreeSet::from([0, 1, 2, 3]);
        let region = ShSampleRegion::new(glam::Vec3::ZERO, glam::Vec3::new(15.0, 3.0, 3.0));
        let mut rows = Vec::with_capacity(16);
        resolve_regions(grid(&nodes), &[region], false, &[&resident], &mut rows).unwrap();
        let warmed_capacity = rows.capacity();
        for _ in 0..32 {
            resolve_regions(grid(&nodes), &[region], false, &[&resident], &mut rows).unwrap();
            assert_eq!(rows.capacity(), warmed_capacity);
        }
    }
}
