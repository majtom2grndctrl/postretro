//! Exact id-41 dense/emitted reconstruction and triangle-envelope scoring.

use super::*;

pub(super) fn passes(
    score: EnvelopeStats,
    magnitude: &MagnitudeStats,
    floor: f32,
    params: &CoarsenParams,
) -> bool {
    score.p95 / magnitude.p95.max(floor) <= params.rel_p95_max
        && score.max / magnitude.max.max(floor) <= params.rel_max_max
}

pub(super) fn score_dense_levels(
    direct: &DirectShDeltaVolumesSection,
    valid_masks: &[u64],
    levels: &[u8],
) -> anyhow::Result<Vec<EnvelopeStats>> {
    anyhow::ensure!(
        levels.len() == valid_masks.len(),
        "id-41 envelope level count mismatch"
    );
    (0..levels.len())
        .map(|cell| {
            let level = Level::from_u8(levels[cell]).ok_or_else(|| {
                anyhow::anyhow!(
                    "id-41 runtime envelope cell {cell} has invalid level {}",
                    levels[cell]
                )
            })?;
            score_dense_cell(direct, valid_masks[cell], cell, level)
        })
        .collect()
}

pub(super) fn score_dense_cell(
    direct: &DirectShDeltaVolumesSection,
    validity: u64,
    cell: usize,
    level: Level,
) -> anyhow::Result<EnvelopeStats> {
    let dense = DenseDirectView::new(direct)?;
    let mut envelope: [Tile; PROBES_PER_CELL] =
        std::array::from_fn(|_| zero_tile(dense.interior_texels));
    for entry in dense.entry_range(cell)? {
        let tiles = dense.decode_valid_entry(entry, validity)?;
        let represented = represent_dense_entry_tiles(&tiles, level, dense.interior_texels);
        for local in bits(validity) {
            let truth = tiles[local].as_ref().expect("valid bit decoded");
            add_abs_residual(
                &mut envelope[local],
                represented[local]
                    .as_ref()
                    .expect("valid target represented"),
                truth,
            );
        }
    }
    Ok(envelope_stats(&envelope, validity))
}

pub(super) fn score_emitted(
    dense: &DirectShDeltaVolumesSection,
    emitted: &DirectShDeltaVolumesSection,
) -> anyhow::Result<Vec<EnvelopeStats>> {
    let dense = DenseDirectView::new(dense)?;
    let emitted = EmittedDeltaSectionRef::from_direct(emitted)?;
    anyhow::ensure!(
        dense.cell_count == emitted.cell_count(),
        "id-41 final envelope cell mismatch"
    );
    let mut output = Vec::with_capacity(dense.cell_count);
    for cell in 0..dense.cell_count {
        let validity = emitted.valid_probe_mask(cell).expect("validated cell");
        let mut envelope: [Tile; PROBES_PER_CELL] =
            std::array::from_fn(|_| zero_tile(dense.interior_texels));
        for entry in dense.entry_range(cell)? {
            let tiles = dense.decode_valid_entry(entry, validity)?;
            let represented = emitted.reconstruct_entry_tiles(cell, entry)?;
            for local in bits(validity) {
                let truth = tiles[local].as_ref().expect("valid bit decoded");
                add_abs_residual(
                    &mut envelope[local],
                    represented[local]
                        .as_ref()
                        .expect("valid target reconstructs"),
                    truth,
                );
            }
        }
        output.push(envelope_stats(&envelope, validity));
    }
    Ok(output)
}

fn represent_dense_entry_tiles(
    tiles: &[Option<Tile>; PROBES_PER_CELL],
    level: Level,
    texels: usize,
) -> [Option<Tile>; PROBES_PER_CELL] {
    let validity = mask_for_tiles(tiles);
    let kept = kept_mask(level, validity);
    let kept_tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|local| {
        (kept & (1u64 << local) != 0).then(|| tiles[local].clone().expect("kept target is valid"))
    });
    let l2 = (level == Level::L2).then(|| {
        reconstruct_l2_tile(tiles, texels)
            .expect("participating L2 cell has a representative")
            .into_iter()
            .map(round_trip_f16)
            .collect::<Tile>()
    });
    std::array::from_fn(|target| {
        (validity & (1u64 << target) != 0).then(|| {
            if level != Level::L2 && kept & (1u64 << target) != 0 {
                return tiles[target].clone().expect("kept target is valid");
            }
            match level {
                Level::L1 => reconstruct_l1_tile(&kept_tiles, target, texels)
                    .unwrap_or_else(|| zero_tile(texels)),
                Level::L2 => l2.as_ref().expect("computed L2 tile").clone(),
                Level::L0 => unreachable!("L0 keeps every valid target"),
            }
        })
    })
}

fn round_trip_f16(value: Vec3) -> Vec3 {
    Vec3::new(
        crate::sh_bake::f16_bits_to_f32(f32_to_f16_bits(value.x)),
        crate::sh_bake::f16_bits_to_f32(f32_to_f16_bits(value.y)),
        crate::sh_bake::f16_bits_to_f32(f32_to_f16_bits(value.z)),
    )
}

fn mask_for_tiles(tiles: &[Option<Tile>; PROBES_PER_CELL]) -> u64 {
    tiles.iter().enumerate().fold(0u64, |mask, (local, tile)| {
        mask | u64::from(tile.is_some()) << local
    })
}

fn add_abs_residual(envelope: &mut Tile, represented: &Tile, truth: &Tile) {
    for ((bound, represented), truth) in envelope.iter_mut().zip(represented).zip(truth) {
        *bound += (*represented - *truth).abs();
    }
}

fn envelope_stats(envelope: &[Tile; PROBES_PER_CELL], validity: u64) -> EnvelopeStats {
    let mut values = Vec::new();
    for local in bits(validity) {
        values.extend(envelope[local].iter().map(|v| v.max_element()));
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let max = values.last().copied().unwrap_or(0.0);
    let p95 = if values.is_empty() {
        0.0
    } else {
        let index = ((values.len() - 1) as f32 * 0.95).round() as usize;
        values[index.min(values.len() - 1)]
    };
    EnvelopeStats {
        p95,
        max,
        samples: values.len() as u64,
    }
}

struct DenseDirectView<'a> {
    offsets: &'a [u32],
    payload: &'a [u16],
    tile_dimension: usize,
    tile_border: usize,
    probe_stride: usize,
    interior_texels: usize,
    cell_count: usize,
}

impl<'a> DenseDirectView<'a> {
    fn new(section: &'a DirectShDeltaVolumesSection) -> anyhow::Result<Self> {
        let cell_count = section.affinity_cell_count();
        anyhow::ensure!(
            section.affinity_offsets.len() == cell_count + 1
                && section.affinity_offsets.first().copied() == Some(0)
                && section
                    .affinity_offsets
                    .windows(2)
                    .all(|pair| pair[0] <= pair[1]),
            "id-41 runtime envelope received invalid dense CSR"
        );
        let tile_dimension = section.tile_dimension as usize;
        let tile_border = section.tile_border as usize;
        anyhow::ensure!(
            tile_dimension > tile_border * 2,
            "id-41 runtime envelope invalid tile geometry"
        );
        let probe_stride = tile_dimension
            .checked_mul(tile_dimension)
            .and_then(|n| n.checked_mul(DELTA_TILE_TEXEL_F16_COUNT))
            .ok_or_else(|| anyhow::anyhow!("id-41 runtime envelope probe stride overflow"))?;
        let entries = section.affinity_offsets.last().copied().unwrap_or(0) as usize;
        let expected = entries
            .checked_mul(PROBES_PER_CELL)
            .and_then(|n| n.checked_mul(probe_stride))
            .ok_or_else(|| anyhow::anyhow!("id-41 runtime envelope dense payload overflow"))?;
        anyhow::ensure!(
            section.delta_subblocks.len() == expected,
            "id-41 runtime envelope needs dense pre-compaction payload: got {} f16 values, expected {expected}",
            section.delta_subblocks.len(),
        );
        let edge = tile_dimension - tile_border * 2;
        Ok(Self {
            offsets: &section.affinity_offsets,
            payload: &section.delta_subblocks,
            tile_dimension,
            tile_border,
            probe_stride,
            interior_texels: edge * edge,
            cell_count,
        })
    }

    fn entry_range(&self, cell: usize) -> anyhow::Result<std::ops::Range<usize>> {
        anyhow::ensure!(
            cell < self.cell_count,
            "id-41 runtime envelope cell {cell} out of range"
        );
        Ok(self.offsets[cell] as usize..self.offsets[cell + 1] as usize)
    }

    fn decode_valid_entry(
        &self,
        entry: usize,
        validity: u64,
    ) -> anyhow::Result<[Option<Tile>; PROBES_PER_CELL]> {
        let mut output: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        for local in bits(validity) {
            let start = entry
                .checked_mul(PROBES_PER_CELL)
                .and_then(|n| n.checked_add(local))
                .and_then(|n| n.checked_mul(self.probe_stride))
                .ok_or_else(|| anyhow::anyhow!("id-41 runtime envelope dense offset overflow"))?;
            let mut tile = Vec::with_capacity(self.interior_texels);
            for y in self.tile_border..self.tile_dimension - self.tile_border {
                for x in self.tile_border..self.tile_dimension - self.tile_border {
                    let i = start + (y * self.tile_dimension + x) * DELTA_TILE_TEXEL_F16_COUNT;
                    anyhow::ensure!(
                        i + 2 < self.payload.len(),
                        "id-41 runtime envelope dense tile is truncated"
                    );
                    tile.push(Vec3::new(
                        crate::sh_bake::f16_bits_to_f32(self.payload[i]),
                        crate::sh_bake::f16_bits_to_f32(self.payload[i + 1]),
                        crate::sh_bake::f16_bits_to_f32(self.payload[i + 2]),
                    ));
                }
            }
            output[local] = Some(tile);
        }
        Ok(output)
    }
}
