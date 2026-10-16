// Reflection-probe batch capture and section assembly: tiles and shares for a batch of probes, then id 54 from the kept ones.
// See: context/lib/resource_management.md §5.2 · build_pipeline.md §PRL section IDs (id 54)

use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_TILE_BORDER,
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeRecord, ReflectionProbesSection,
};
use rayon::prelude::*;

use crate::bake_control::BakeControl;

use super::capture::{RowCapture, TILE_INTERIOR, capture_row, encode_distance_tile, encode_tile};
use super::placement::{PlacedProbe, bindings};
use super::shading::CaptureScene;
use super::share::{ProbeShares, ShareReport, ShareSums};
use super::split::StageProbe;

/// One probe's capture: its encoded colour and distance tiles and its share.
pub(crate) struct ProbeCapture {
    pub tile: Vec<u8>,
    pub distance: Vec<u8>,
    pub shares: ProbeShares,
}

/// Capture each of `probes`' colour and distance tiles and light share. Work
/// items are tile rows, so one probe spreads across workers; rows land in
/// item order and each probe's share sums merge in row order, so the bytes
/// never depend on the worker count. `published` counts the rows announced
/// to the progress display so far.
pub(crate) fn capture_probes(
    scene: &CaptureScene<'_>,
    probes: &[StageProbe],
    references: &[[f32; 3]],
    control: &BakeControl,
    published: &mut usize,
) -> anyhow::Result<Vec<ProbeCapture>> {
    let rows_per_tile = TILE_INTERIOR as usize;
    let items: Vec<(usize, u32)> = (0..probes.len())
        .flat_map(|probe| (0..TILE_INTERIOR).map(move |row| (probe, row)))
        .collect();
    *published += items.len();
    control.publish_total(*published);
    let rows = items
        .par_iter()
        .map(|&(probe, row)| {
            let _permit = control.governor().enter();
            let point = probes[probe].placed.capture_point_vec3();
            let captured = capture_row(scene, point, row, references);
            control.advance(1);
            captured
        })
        .collect::<anyhow::Result<Vec<RowCapture>>>()?;
    Ok(rows
        .par_chunks(rows_per_tile)
        .map(|rows| {
            let texels: Vec<&[glam::Vec3]> = rows.iter().map(|r| r.texels.as_slice()).collect();
            let distances: Vec<&[f32]> = rows.iter().map(|r| r.distances.as_slice()).collect();
            let mut sums = ShareSums::default();
            for row in rows {
                sums.merge(&row.shares);
            }
            ProbeCapture {
                tile: encode_tile(&texels),
                distance: encode_distance_tile(&distances),
                shares: sums.finish(references),
            }
        })
        .collect())
}

/// Id 54 from the kept probes, in their order, each on its own layer. A
/// dropped probe's capture is discarded.
pub(crate) fn assemble_section(
    probes: &[StageProbe],
    captures: Vec<ProbeCapture>,
    kept: &[usize],
) -> (ReflectionProbesSection, Vec<ShareReport>) {
    let mut keep = vec![false; probes.len()];
    for &i in kept {
        keep[i] = true;
    }
    let captures: Vec<ProbeCapture> = captures
        .into_iter()
        .zip(&keep)
        .filter_map(|(capture, &kept)| kept.then_some(capture))
        .collect();
    let placed: Vec<PlacedProbe> = kept.iter().map(|&i| probes[i].placed.clone()).collect();
    let section = ReflectionProbesSection {
        tile_dimension: REFLECTION_PROBE_TILE_DIMENSION,
        tile_border: REFLECTION_PROBE_TILE_BORDER,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        probes: placed
            .iter()
            .zip(&captures)
            .enumerate()
            .map(|(layer, (probe, capture))| ReflectionProbeRecord {
                source_entity: probe.source_entity,
                box_min: probe.box_min,
                box_max: probe.box_max,
                capture_point: probe.capture_point,
                layer: layer as u32,
                lights: capture.shares.entries,
            })
            .collect(),
        bindings: bindings(&placed),
        tiles: captures
            .iter()
            .flat_map(|c| c.tile.iter().copied())
            .collect(),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: captures
            .iter()
            .flat_map(|c| c.distance.iter().copied())
            .collect(),
    };
    let reports = captures.iter().map(|c| c.shares.report).collect();
    (section, reports)
}
