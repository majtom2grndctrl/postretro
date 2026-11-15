// Test fixtures for mover depth live tiles: closed-pose tiles and depth-only
// id-57 sections, shared by the CPU plan tests and the GPU tests.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    DepthRecordParts, MoverDepthRecord, MoverProbeChannelParts, MoverProbeChannelSection,
    MoverRecord, PoseMark,
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
                    record: MoverDepthRecord::termini(
                        mover_id,
                        probe_index,
                        tiles.each_ref().map(Option::is_some),
                    ),
                    tiles: tiles.into_iter().flatten().flatten().collect(),
                })
                .collect(),
        },
        |_| u64::MAX,
        |mover_id| Ok::<_, ()>(MoverRecord::termini(mover_id, 0.0, 1.0)),
    )
    .unwrap()
    .expect("depth records make a section")
}

/// What one pose of a posed depth record holds for its probe.
#[derive(Clone)]
pub(in crate::render) enum PosedDepth {
    Tile(Vec<[u16; 2]>),
    /// A mid pose marked base (or an unstored terminus): the probe's base.
    Base,
    /// An unmarked mid pose: blended across.
    Across,
}

/// A mover's pose table for a posed fixture: id, ramp `(s, e)`, knots.
pub(in crate::render) type PosedMover = (u32, (f32, f32), Vec<f32>);

/// An id-57 section carrying only depth over movers with any number of
/// poses: each record is `(mover, probe, one PosedDepth per pose)`.
pub(in crate::render) fn posed_depth_section(
    movers: &[PosedMover],
    records: Vec<(u32, u32, Vec<PosedDepth>)>,
) -> MoverProbeChannelSection {
    MoverProbeChannelSection::assemble(
        MoverProbeChannelParts {
            affinity_dims: [1, 1, 1],
            direct: Vec::new(),
            bounce: Vec::new(),
            depth: records
                .into_iter()
                .map(|(mover_id, probe_index, poses)| {
                    let last = poses.len() - 1;
                    DepthRecordParts {
                        record: MoverDepthRecord {
                            mover_id,
                            probe_index,
                            marks: poses
                                .iter()
                                .enumerate()
                                .map(|(pose, state)| match state {
                                    PosedDepth::Tile(_) => PoseMark::Stored,
                                    PosedDepth::Base if pose != 0 && pose != last => PoseMark::Base,
                                    _ => PoseMark::Unmarked,
                                })
                                .collect(),
                        },
                        tiles: poses
                            .into_iter()
                            .filter_map(|state| match state {
                                PosedDepth::Tile(tile) => Some(tile),
                                _ => None,
                            })
                            .flatten()
                            .collect(),
                    }
                })
                .collect(),
        },
        |_| u64::MAX,
        |mover_id| {
            let &(_, (ramp_start, ramp_end), ref knots) = movers
                .iter()
                .find(|mover| mover.0 == mover_id)
                .expect("every record's mover is listed");
            Ok::<_, ()>(MoverRecord {
                mover_id,
                ramp_start,
                ramp_end,
                knots: knots.clone(),
            })
        },
    )
    .unwrap()
    .expect("depth records make a section")
}

/// One moment of a two-pose (termini-only) mover's live-tile blend, exactly,
/// not a mid-pose interval: `D₀` toward the probe's base over t′ ∈ [0, 0.5],
/// the base toward `D₁` over [0.5, 1] (owner, 2026-11-09).
pub(in crate::render) fn through_base(d0: f32, base: f32, d1: f32, t: f32) -> f32 {
    if t <= 0.5 {
        d0 + 2.0 * t * (base - d0)
    } else {
        base + (2.0 * t - 1.0) * (d1 - base)
    }
}
