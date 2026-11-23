// Reflection-probe section capture: every probe's tiles and light share, traced row-parallel and assembled into id 54.
// See: context/lib/resource_management.md §5.2 · build_pipeline.md §PRL section IDs (id 54)

use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_TILE_BORDER,
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeRecord, ReflectionProbesSection,
};
use rayon::prelude::*;

use crate::bake_control::BakeControl;

use super::capture::{RowCapture, TILE_INTERIOR, capture_row, encode_distance_tile, encode_tile};
use super::capture_exclusion::ProbeExclusion;
use super::placement::{PlacedProbe, fluid_bindings};
use super::shading::CaptureScene;
use super::share::{ShareReport, ShareSums};

/// [`capture_probes`] with each probe's primary rays skipping its
/// `excluded` brushes (sorted brush indices, one set per probe).
#[cfg(test)]
pub(super) fn capture_section(
    scene: &CaptureScene<'_>,
    probes: &[PlacedProbe],
    excluded: &[Vec<u32>],
    references: &[[f32; 3]],
    control: &BakeControl,
) -> anyhow::Result<(ReflectionProbesSection, Vec<ShareReport>)> {
    anyhow::ensure!(
        excluded.len() == probes.len(),
        "{} excluded set(s) for {} probe(s)",
        excluded.len(),
        probes.len()
    );
    let exclusions: Vec<ProbeExclusion<'_>> = probes
        .iter()
        .zip(excluded)
        .map(|(probe, excluded)| ProbeExclusion::new(&scene.tracer, scene.brushes, excluded, probe))
        .collect();
    capture_probes(scene, probes, &exclusions, references, control)
}

/// Capture every probe's colour and distance tiles and light share, each
/// probe's primary rays under its exclusion (one per probe). Work items are
/// tile rows, so one probe spreads across workers; rows land in item order
/// and each probe's share sums merge in row order, so the bytes never depend
/// on the worker count.
pub(super) fn capture_probes(
    scene: &CaptureScene<'_>,
    probes: &[PlacedProbe],
    exclusions: &[ProbeExclusion<'_>],
    references: &[[f32; 3]],
    control: &BakeControl,
) -> anyhow::Result<(ReflectionProbesSection, Vec<ShareReport>)> {
    anyhow::ensure!(
        exclusions.len() == probes.len(),
        "{} exclusion(s) for {} probe(s)",
        exclusions.len(),
        probes.len()
    );
    let rows_per_tile = TILE_INTERIOR as usize;
    let items: Vec<(usize, u32)> = (0..probes.len())
        .flat_map(|probe| (0..TILE_INTERIOR).map(move |row| (probe, row)))
        .collect();
    control.publish_total(items.len());
    let rows = items
        .par_iter()
        .map(|&(probe, row)| {
            let _permit = control.governor().enter();
            let captured = capture_row(
                scene,
                probes[probe].capture_point_vec3(),
                &exclusions[probe],
                row,
                references,
            );
            control.advance(1);
            captured
        })
        .collect::<anyhow::Result<Vec<RowCapture>>>()?;
    let tiles: Vec<Vec<u8>> = rows
        .par_chunks(rows_per_tile)
        .map(|rows| {
            let _permit = control.governor().enter();
            let texels: Vec<&[glam::Vec3]> = rows.iter().map(|r| r.texels.as_slice()).collect();
            encode_tile(&texels)
        })
        .collect();
    let distances: Vec<u8> = rows
        .chunks(rows_per_tile)
        .flat_map(|rows| {
            let texels: Vec<&[f32]> = rows.iter().map(|r| r.distances.as_slice()).collect();
            encode_distance_tile(&texels)
        })
        .collect();
    let shares: Vec<_> = rows
        .chunks(rows_per_tile)
        .map(|rows| {
            let mut sums = ShareSums::default();
            for row in rows {
                sums.merge(&row.shares);
            }
            sums.finish(references)
        })
        .collect();

    let section = ReflectionProbesSection {
        tile_dimension: REFLECTION_PROBE_TILE_DIMENSION,
        tile_border: REFLECTION_PROBE_TILE_BORDER,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        // Mip 0 alone until the prefilter writes the chain.
        colour_mip_count: 1,
        probes: probes
            .iter()
            .zip(&shares)
            .enumerate()
            .map(|(layer, (probe, share))| ReflectionProbeRecord {
                source_entity: probe.source_entity,
                box_min: probe.box_min,
                box_max: probe.box_max,
                capture_point: probe.capture_point,
                layer: layer as u32,
                lights: share.entries,
            })
            .collect(),
        fluid_bindings: fluid_bindings(probes),
        // No world face binds until glossy placement lands.
        world_bindings: Vec::new(),
        tiles: tiles.concat(),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances,
    };
    Ok((section, shares.iter().map(|share| share.report).collect()))
}
