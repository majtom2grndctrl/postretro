// Test fixtures for mover depth live tiles: closed-pose tiles and depth-only
// id-57 sections, shared by the CPU plan tests and the GPU tests.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    DepthRecordParts, MoverDepthRecord, MoverLightRampRecord, MoverProbeChannelParts,
    MoverProbeChannelSection,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

/// A tile whose every texel holds `[mean, mean_sq]` as f16 bits.
pub(in crate::render) fn uniform_tile(mean: f32, mean_sq: f32) -> Vec<[u16; 2]> {
    vec![[f32_to_f16_bits(mean), f32_to_f16_bits(mean_sq)]; PROBE_DEPTH_TILE_TEXELS]
}

/// A tile whose texels differ: texel `i` holds `[base + i/64, (base + i/64)² + 1]`.
pub(in crate::render) fn ramp_tile(base: f32) -> Vec<[u16; 2]> {
    (0..PROBE_DEPTH_TILE_TEXELS)
        .map(|i| {
            let mean = base + i as f32 / 64.0;
            [f32_to_f16_bits(mean), f32_to_f16_bits(mean * mean + 1.0)]
        })
        .collect()
}

/// One depth record: mover id, probe index, `D₀`, `D₁`.
pub(in crate::render) type DepthFixtureRecord = (u32, u32, Vec<[u16; 2]>, Vec<[u16; 2]>);

/// One depth record storing only some termini: mover id, probe index, and
/// `[D₀, D₁]`, `None` where the record stores no tile and the probe reads
/// its base.
pub(in crate::render) type StoredDepthRecord = (u32, u32, [Option<Vec<[u16; 2]>>; 2]);

/// An id-57 section carrying only depth, every record storing both termini.
pub(in crate::render) fn depth_section(
    records: Vec<DepthFixtureRecord>,
) -> MoverProbeChannelSection {
    stored_depth_section(
        records
            .into_iter()
            .map(|(mover_id, probe, d0, d1)| (mover_id, probe, [Some(d0), Some(d1)]))
            .collect(),
    )
}

/// An id-57 section carrying only depth, each record storing the termini it
/// names.
pub(in crate::render) fn stored_depth_section(
    records: Vec<StoredDepthRecord>,
) -> MoverProbeChannelSection {
    MoverProbeChannelSection::assemble(
        MoverProbeChannelParts {
            affinity_dims: [1, 1, 1],
            direct: Vec::new(),
            bounce: Vec::new(),
            depth: records
                .into_iter()
                .map(|(mover_id, probe_index, tiles)| DepthRecordParts {
                    record: MoverDepthRecord {
                        mover_id,
                        probe_index,
                        stored_termini: tiles.each_ref().map(Option::is_some),
                    },
                    tiles: tiles.into_iter().flatten().flatten().collect(),
                })
                .collect(),
        },
        |_| u64::MAX,
        |mover_id| {
            Ok::<_, ()>(MoverLightRampRecord {
                mover_id,
                start: 0.0,
                end: 1.0,
            })
        },
    )
    .unwrap()
    .expect("depth records make a section")
}
