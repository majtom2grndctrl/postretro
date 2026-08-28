use super::*;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_volume::OctahedralShProbe;

const TILE: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;
const BORDER: u32 = DEFAULT_IRRADIANCE_TILE_BORDER;

fn dense_direct(
    cells: usize,
    entries_per_cell: usize,
    value: impl Fn(usize, usize) -> f32,
) -> DirectShDeltaVolumesSection {
    let stride = TILE as usize * TILE as usize * DELTA_TILE_TEXEL_F16_COUNT;
    let mut payload = Vec::new();
    let mut offsets = Vec::with_capacity(cells + 1);
    let mut lights = Vec::new();
    offsets.push(0);
    for _ in 0..cells {
        for entry in 0..entries_per_cell {
            lights.push(entry as u32);
            for local in 0..PROBES_PER_CELL {
                let v = value(entry, local);
                for _ in 0..TILE as usize * TILE as usize {
                    payload.extend([v, v, v].map(f32_to_f16_bits));
                    payload.push(f32_to_f16_bits(1.0));
                }
            }
        }
        offsets.push(lights.len() as u32);
    }
    assert_eq!(
        payload.len(),
        cells * entries_per_cell * PROBES_PER_CELL * stride
    );
    DirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR as u8,
        affinity_dims: [cells as u32, 1, 1],
        tile_dimension: TILE,
        tile_border: BORDER,
        valid_probe_masks: vec![u64::MAX; cells],
        cell_levels: vec![Level::L0.to_u8(); cells],
        affinity_offsets: offsets,
        affinity_lights: lights,
        delta_subblocks: payload,
    }
}

fn bright_base(cells: usize, value: f32) -> OctahedralShVolumeSection {
    let dims = [cells as u32 * AFFINITY_FACTOR, 4, 4];
    let total = dims.iter().map(|&v| v as usize).product::<usize>();
    let mut base = OctahedralShVolumeSection::placeholder();
    base.grid_origin = [0.0; 3];
    base.cell_size = [1.0; 3];
    base.grid_dimensions = dims;
    base.tile_dimension = TILE;
    base.tile_border = BORDER;
    base.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    base.compact_atlas_dimensions = [total as u32 * TILE, TILE];
    base.compact_atlas_tiles_per_row = total as u32;
    base.compact_atlas_tiles_per_layer = total as u32;
    base.compact_atlas_layer_count = 1;
    base.probes = vec![
        OctahedralShProbe {
            validity: 1,
            ..Default::default()
        };
        total
    ];
    let half = f32_to_f16_bits(value).to_le_bytes();
    base.compact_atlas = vec![0; total * TILE as usize * TILE as usize * 8];
    for tile in 0..total {
        for y in 0..TILE as usize {
            for x in 0..TILE as usize {
                let pixel = y * total * TILE as usize + tile * TILE as usize + x;
                let byte = pixel * 8;
                for channel in 0..3 {
                    base.compact_atlas[byte + channel * 2] = half[0];
                    base.compact_atlas[byte + channel * 2 + 1] = half[1];
                }
            }
        }
    }
    base
}

#[test]
fn independent_entries_use_triangle_envelope_not_unit_sum_cancellation() {
    let direct = dense_direct(1, 2, |entry, local| {
        let ramp = local as f32;
        if entry == 0 { ramp } else { -ramp }
    });
    let score = score_dense_cell(&direct, u64::MAX, 0, Level::L2).unwrap();
    assert!(
        score.max > 50.0,
        "opposing entries must add in the absolute envelope"
    );
}

#[test]
fn l2_virtual_score_matches_final_f16_emitted_score() {
    let mut direct = dense_direct(1, 1, |_, local| local as f32 * 0.0137);
    direct.cell_levels[0] = Level::L2.to_u8();
    let mut base = OctahedralShVolumeSection::placeholder();
    base.grid_dimensions = [4, 4, 4];
    base.probes = vec![
        OctahedralShProbe {
            validity: 1,
            ..Default::default()
        };
        PROBES_PER_CELL
    ];
    let masks = base_valid_probe_masks(&base).unwrap();
    let virtual_score = score_dense_levels(&direct, &masks, &direct.cell_levels).unwrap()[0];
    let compacted = compact_direct_valid_probes(&direct, &base).unwrap();
    let emitted_score = score_emitted(&direct, &compacted).unwrap()[0];
    assert!(
        (virtual_score.p95 - emitted_score.p95).abs() < 1.0e-6,
        "virtual {virtual_score:?}, emitted {emitted_score:?}"
    );
    assert!(
        (virtual_score.max - emitted_score.max).abs() < 1.0e-6,
        "virtual {virtual_score:?}, emitted {emitted_score:?}"
    );
}

#[test]
fn l1_virtual_score_matches_final_sparse_corner_emitted_score() {
    let mut direct = dense_direct(1, 1, |_, local| {
        let (x, y, z) = postretro_level_format::sh_reconstruct::local_xyz(local);
        ((x + y * 3 + z * 7) % 5) as f32 * 0.137
    });
    direct.cell_levels[0] = Level::L1.to_u8();
    let mut base = OctahedralShVolumeSection::placeholder();
    base.grid_dimensions = [4, 4, 4];
    base.probes = vec![
        OctahedralShProbe {
            validity: 1,
            ..Default::default()
        };
        PROBES_PER_CELL
    ];
    let masks = base_valid_probe_masks(&base).unwrap();
    let virtual_score = score_dense_levels(&direct, &masks, &direct.cell_levels).unwrap()[0];
    let compacted = compact_direct_valid_probes(&direct, &base).unwrap();
    let emitted_score = score_emitted(&direct, &compacted).unwrap()[0];
    assert!(
        virtual_score.max > 0.0,
        "fixture must exercise dropped probes"
    );
    assert!(
        (virtual_score.p95 - emitted_score.p95).abs() < 1.0e-6,
        "virtual {virtual_score:?}, emitted {emitted_score:?}"
    );
    assert!(
        (virtual_score.max - emitted_score.max).abs() < 1.0e-6,
        "virtual {virtual_score:?}, emitted {emitted_score:?}"
    );
}

#[test]
fn seam_smoothing_uses_envelope_valid_l1_or_falls_back_to_l0() {
    let mut direct = dense_direct(2, 1, |_, local| (local % 2) as f32 * 10.0);
    direct.cell_levels = vec![Level::L0.to_u8(), Level::L2.to_u8()];
    let masks = vec![u64::MAX; 2];
    let magnitudes = vec![
        MagnitudeStats {
            p95: 1.0,
            max: 1.0,
            ..Default::default()
        };
        2
    ];
    let mut refined = vec![false; 2];
    smooth_to_fixpoint(
        &mut direct,
        &masks,
        &magnitudes,
        1.0e-6,
        &CoarsenParams::default(),
        &mut refined,
    )
    .unwrap();
    assert_eq!(direct.cell_levels, vec![0, 0], "failing L1 must be skipped");
    assert!(refined[1]);
}

#[test]
fn mutable_cost_is_hypothetical_and_counts_whole_affected_cells() {
    let offsets = [0, 2, 3];
    let lights = [0, 1, 0];
    let levels = [Level::L2.to_u8(), Level::L2.to_u8()];
    let masks = [u64::MAX, u64::MAX];
    let cost = mutable_cost(
        27,
        &offsets,
        &lights,
        &levels,
        &masks,
        4,
        |slot| slot == 1,
        1,
    );
    assert_eq!(cost.affected_cells, 1);
    assert_eq!(cost.affected_entries, 1);
    assert!(cost.forced_l0_payload_bytes > cost.current_payload_bytes);
    assert!(cost.forced_l0_payload_bytes < cost.uniform_l0_payload_bytes);
}

#[test]
fn participating_i5_ignores_zero_valid_sentinels() {
    validate_participating_i5(&[0, 2], &[u64::MAX, 0], [2, 1, 1]).unwrap();
    assert!(validate_participating_i5(&[0, 2], &[u64::MAX; 2], [2, 1, 1]).is_err());
}

#[test]
fn controller_restores_a_failing_direct_cell_and_exactly_revalidates() {
    let base = bright_base(1, 100.0);
    let mut direct = dense_direct(1, 1, |_, local| {
        let (x, y, z) = postretro_level_format::sh_reconstruct::local_xyz(local);
        if (x + y + z) % 2 == 0 { 100.0 } else { -100.0 }
    });
    direct.cell_levels[0] = Level::L2.to_u8();
    let mut sections =
        PostBakeDeltaSections::new(Default::default(), None, None, Some(direct), None);
    let report = apply_runtime_safe_envelope(
        &base,
        None,
        &mut sections,
        &ScriptMutableDescriptorSlots::empty(0),
        &CoarsenParams::default(),
    )
    .unwrap();
    assert_eq!(report.failures_before_repair, 1);
    assert_eq!(report.failures_after_repair, 0);
    assert_eq!(report.selected_l0_restores, 1);
    assert_eq!(
        sections.direct.unwrap().cell_levels,
        vec![Level::L0.to_u8()]
    );
}
