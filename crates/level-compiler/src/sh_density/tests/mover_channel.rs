// Bake row B9: every base cell under a mover probe channel entry stores L0,
// and every other cell keeps the level the same classifier picks with the
// channel withheld, save the cells seam smoothing refines beside a capped
// one.

use postretro_level_format::mover_probe_channel::{
    MoverBounceEntry, MoverBouncePlacement, MoverDirectEntry, MoverLightNamespace,
    MoverLightRampRecord, MoverProbeChannelSection,
};

use super::*;

/// A channel over `affinity_dims` with one direct entry at each of `direct`
/// and one bounce entry at each of `bounce`, every probe valid.
fn channel(
    affinity_dims: [u32; 3],
    direct: &[usize],
    bounce: &[usize],
) -> MoverProbeChannelSection {
    let cells = affinity_dims
        .iter()
        .map(|&dim| dim as usize)
        .product::<usize>();
    let offsets = |at: &[usize]| {
        let mut offsets = vec![0u32; cells + 1];
        for &cell in at {
            offsets[cell + 1] += 1;
        }
        for cell in 0..cells {
            offsets[cell + 1] += offsets[cell];
        }
        offsets
    };
    MoverProbeChannelSection {
        affinity_factor: 4,
        affinity_dims,
        bounce_placement: MoverBouncePlacement::Resident,
        valid_probe_masks: vec![u64::MAX; cells],
        direct_offsets: offsets(direct),
        direct_entries: direct
            .iter()
            .map(|_| MoverDirectEntry {
                mover_id: 0,
                namespace: MoverLightNamespace::StaticLight,
                carries_own_tile: false,
                light_index: 0,
            })
            .collect(),
        visibility: vec![255; direct.len() * 128],
        own_tiles: Vec::new(),
        bounce_offsets: offsets(bounce),
        bounce_entries: bounce
            .iter()
            .map(|_| MoverBounceEntry {
                mover_id: 0,
                affected_probe_mask: 1,
            })
            .collect(),
        bounce_tiles: vec![0; bounce.len() * 216],
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps: vec![MoverLightRampRecord {
            mover_id: 0,
            start: 0.0,
            end: 1.0,
        }],
    }
}

#[test]
fn storage_ceilings_cap_every_cell_holding_a_direct_or_bounce_entry() {
    let section = channel([3, 1, 1], &[0], &[2]);
    let with = storage_level_ceilings(
        [12, 4, 4],
        DeltaSectionsRef {
            mover_channel: Some(&section),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(with, vec![Level::L0, Level::L2, Level::L0]);
    let without = storage_level_ceilings([12, 4, 4], DeltaSectionsRef::default()).unwrap();
    assert_eq!(without, vec![Level::L2; 3]);

    let misaligned = channel([2, 1, 1], &[0], &[]);
    assert!(
        storage_level_ceilings(
            [12, 4, 4],
            DeltaSectionsRef {
                mover_channel: Some(&misaligned),
                ..Default::default()
            },
        )
        .is_err()
    );
}

#[test]
fn base_levels_move_only_under_channel_cells_and_where_smoothing_refines_beside_them() {
    // A 4×4×4-brick grid whose probes carry a smooth field: the classifier
    // coarsens most bricks, so caps and the seams they force show.
    let dimensions = [16, 16, 16];
    let base = raw_indirect(dimensions, |_| true);
    let affinity = [4, 4, 4];
    // Cell 61 sits beside the field's two L2 bricks, so capping it forces a
    // seam refinement.
    let capped = [21usize, 61, 42];
    let section = channel(affinity, &capped[..2], &capped[2..]);
    let params = CoarsenParams::default();
    let without =
        classify_base_levels(&base, None, DeltaSectionsRef::default(), &[], &params).unwrap();
    let with = classify_base_levels(
        &base,
        None,
        DeltaSectionsRef {
            mover_channel: Some(&section),
            ..Default::default()
        },
        &[],
        &params,
    )
    .unwrap();
    assert!(
        without.levels.iter().any(|&level| level != Level::L0),
        "the fixture must coarsen somewhere: {:?}",
        without.levels
    );
    let neighbours = |cell: usize| {
        let (x, y, z) = (cell % 4, cell / 4 % 4, cell / 16);
        let mut around = Vec::new();
        for (dx, dy, dz) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
            for sign in [-1i64, 1] {
                let (nx, ny, nz) = (
                    x as i64 + sign * dx,
                    y as i64 + sign * dy,
                    z as i64 + sign * dz,
                );
                if (0..4).contains(&nx) && (0..4).contains(&ny) && (0..4).contains(&nz) {
                    around.push((nx + ny * 4 + nz * 16) as usize);
                }
            }
        }
        around
    };
    let mut refined = 0;
    for cell in 0..64 {
        if capped.contains(&cell) {
            assert_eq!(with.levels[cell], Level::L0, "capped cell {cell}");
            assert_eq!(with.scales[cell], 0, "capped cell {cell} joins no node");
            continue;
        }
        if with.levels[cell] == without.levels[cell] {
            continue;
        }
        assert!(
            neighbours(cell).iter().any(|other| capped.contains(other))
                && with.levels[cell].to_u8() < without.levels[cell].to_u8(),
            "cell {cell} moved from {:?} to {:?} with no capped neighbour forcing it",
            without.levels[cell],
            with.levels[cell],
        );
        refined += 1;
    }
    assert!(
        refined > 0,
        "the cap beside an L2 brick forces a seam refinement"
    );
    let pinned = capped
        .iter()
        .filter(|&&cell| without.levels[cell] != Level::L0)
        .count() as u64;
    assert!(
        pinned > 0,
        "a capped cell must have coarsened without the channel"
    );
    assert_eq!(with.delta_pins[3], pinned);
}
