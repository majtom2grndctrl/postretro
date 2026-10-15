//! Reflection-probe capture stage (PRL id 54): place authored and derived
//! probes over up-facing fluid faces, capture each probe's static world into
//! an octahedral colour tile and a distance tile.
//! See: context/lib/resource_management.md §5 · build_pipeline.md §Compiler pipeline, §Build Cache

mod animated_direct;
mod authored;
mod box_fit;
mod capture;
mod capture_point;
mod capture_visibility;
mod delta_sample;
mod derived;
mod hit;
mod lightmap_sample;
mod materials;
mod placement;
mod reference;
mod sdf_lights;
mod sh_sample;
mod shading;
mod share;
mod stage_key;

#[cfg(test)]
mod capture_tests;
#[cfg(test)]
mod distance_tests;
#[cfg(test)]
mod placement_test_fixtures;
#[cfg(test)]
mod placement_tests;
#[cfg(test)]
mod reflection_probes_test_fixtures;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use bvh::bvh::Bvh;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapSection};
use postretro_level_format::reflection_probes::{
    MAX_REFLECTION_PROBES, REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
    REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_TILE_BORDER,
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeRecord, ReflectionProbesSection,
};
use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::StageCache;
use crate::geometry::GeometryResult;
use crate::map_data::MapReflectionVolume;
use crate::partition::BspTree;

use animated_direct::AnimatedDirectSampler;
use box_fit::WorldBounds;
use capture::{RowCapture, TILE_INTERIOR, capture_row, encode_distance_tile, encode_tile};
use capture_point::PointClassifier;
use capture_visibility::warn_hidden_water;
use delta_sample::DeltaShSampler;
use hit::SceneTracer;
use lightmap_sample::LightmapSampler;
use materials::SurfaceMaterials;
use placement::{PlacedProbe, PlacementScene, bindings, place_probes, up_facing_faces};
use reference::reference_value;
use sdf_lights::SdfLights;
use sh_sample::{PRODUCTION_PROBE_OCCLUSION, ShVolumeSampler};
use shading::CaptureScene;
use share::{ShareReport, ShareSums};
use stage_key::{cache_key, cache_payload, parse_cache_payload};

/// Cache stage identity for the whole id-54 section.
const STAGE_ID: &str = "reflection_probes";
/// Bump when capture shading, sampling or the payload changes. Placement
/// enters the key as its output, so a placement change re-keys by itself.
/// 3: animated direct and indirect at reference, sdf lights, light share.
/// 4: the distance tile (mean inverse L1 hit distance per texel).
const STAGE_VERSION: u32 = 4;

/// What the stage reads. Everything here enters its cache key.
pub(crate) struct ReflectionProbeInputs<'a> {
    /// The static world: movers and environment volumes are not in it.
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    /// Id 52, whose up-facing fluid faces bind the probes and whose fluid
    /// brushes drop a capture point inside them.
    pub environment_volumes: &'a EnvironmentVolumesSection,
    /// Authored boxes, in map entity order.
    pub reflection_volumes: &'a [MapReflectionVolume],
    /// The compiler BSP and its exterior leaves: a capture point in a solid
    /// leaf is in solid, one in an exterior leaf is outside the hull.
    pub tree: &'a BspTree,
    pub exterior_leaves: &'a HashSet<usize>,
    /// Id 22 as encoded: the capture decodes the texels it reads.
    pub lightmap: &'a LightmapSection,
    /// The packed pre-BC6H base SH volume id 34 encodes. Warm SH differs from
    /// cold, so the key hashes this output, not the SH bake's inputs. Its
    /// animation descriptors are the runtime's, by roster index.
    pub sh_volume: &'a OctahedralShVolumeSection,
    /// Id 27 as the level ships it: each animated light's bounce.
    pub delta_sh: Option<&'a DeltaShVolumesSection>,
    /// Id 25 as the level ships it: each animated light's direct.
    pub animated_weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    /// Id 18: the runtime's lights, whose non-dynamic records are the spec
    /// lights the forward sdf loop shades.
    pub alpha_lights: &'a AlphaLightsSection,
    /// Id 23: the chunk windows the sdf selection reads.
    pub chunk_light_list: &'a ChunkLightListSection,
    /// Texture name → `.prm` key, from the TextureMips stage.
    pub texture_keys: &'a HashMap<String, [u8; 32]>,
    pub prm_root: &'a Path,
}

/// Id 54 for the level, from the cache when its key hits; `None` when no
/// probe is kept, so the section is omitted.
pub(crate) fn bake_or_load_reflection_probes(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    if inputs.reflection_volumes.is_empty()
        && up_facing_faces(inputs.environment_volumes).is_empty()
    {
        return Ok(None);
    }
    let Some(bounds) = WorldBounds::from_positions(
        inputs
            .geometry
            .geometry
            .vertices
            .iter()
            .map(|v| &v.position),
    ) else {
        log::warn!("[ReflectionProbes] the level has no static world to reflect; no probes");
        return Ok(None);
    };
    let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
    // Placement runs on every build, warm included, so its drop warnings
    // always reach the mapper; its output keys the capture.
    let classifier = PointClassifier::new(
        inputs.tree,
        inputs.exterior_leaves,
        inputs.environment_volumes,
    );
    let probes = place_probes(
        inputs.environment_volumes,
        inputs.reflection_volumes,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        MAX_REFLECTION_PROBES,
    )?;
    if probes.is_empty() {
        return Ok(None);
    }

    // Capture is a pure function of the keyed inputs, so a hit skips it.
    let key = cache_key(inputs, &probes)?;
    if let Some((section, reports)) =
        stage_cache
            .and_then(|cache| cache.get(&key))
            .and_then(|bytes| {
                parse_cache_payload(&bytes)
                    .map_err(|e| {
                        log::warn!("[cache] corrupt reflection_probes entry, re-baking: {e}")
                    })
                    .ok()
            })
    {
        log::info!("[cache] reflection_probes hit");
        log_probes(&section, &reports);
        warn_hidden_water(&section, &probes, inputs.environment_volumes);
        return Ok(Some(section));
    }
    log::info!("[cache] reflection_probes miss");

    let sh = ShVolumeSampler::new(inputs.sh_volume, PRODUCTION_PROBE_OCCLUSION)?;
    let deltas = inputs
        .delta_sh
        .map(|delta| {
            DeltaShSampler::new(
                delta,
                &sh,
                inputs.sh_volume.tile_dimension,
                inputs.sh_volume.grid_dimensions,
            )
        })
        .transpose()?;
    let references = references(&inputs.sh_volume.animation_descriptors);
    check_roster(inputs, references.len())?;
    let scene = CaptureScene {
        tracer,
        materials: SurfaceMaterials::new(
            &inputs.geometry.texture_names.names,
            inputs.texture_keys,
            inputs.prm_root,
        ),
        lightmap: LightmapSampler::new(inputs.lightmap),
        sh,
        animated_direct: inputs
            .animated_weight_maps
            .map(|maps| AnimatedDirectSampler::new(maps, inputs.lightmap)),
        deltas,
        sdf: SdfLights::new(inputs.alpha_lights, inputs.chunk_light_list),
        max_distance: ((bounds.max - bounds.min).length() + 1.0) as f32,
    };
    let (section, reports) = capture_section(&scene, &probes, &references, control)?;
    if let Some(cache) = stage_cache {
        cache.put(&key, &cache_payload(&section, &reports));
    }
    log_probes(&section, &reports);
    warn_hidden_water(&section, &probes, inputs.environment_volumes);
    Ok(Some(section))
}

/// Capture every probe's colour and distance tiles and light share. Work
/// items are tile rows, so one probe spreads across workers; rows land in
/// item order and each probe's share sums merge in row order, so the bytes
/// never depend on the worker count.
fn capture_section(
    scene: &CaptureScene<'_>,
    probes: &[PlacedProbe],
    references: &[[f32; 3]],
    control: &BakeControl,
) -> anyhow::Result<(ReflectionProbesSection, Vec<ShareReport>)> {
    let rows_per_tile = TILE_INTERIOR as usize;
    let items: Vec<(usize, u32)> = (0..probes.len())
        .flat_map(|probe| (0..TILE_INTERIOR).map(move |row| (probe, row)))
        .collect();
    control.publish_total(items.len());
    let rows = items
        .par_iter()
        .map(|&(probe, row)| {
            let _permit = control.governor().enter();
            let captured = capture_row(scene, probes[probe].capture_point_vec3(), row, references);
            control.advance(1);
            captured
        })
        .collect::<anyhow::Result<Vec<RowCapture>>>()?;
    let tiles: Vec<Vec<u8>> = rows
        .par_chunks(rows_per_tile)
        .map(|rows| {
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
        bindings: bindings(probes),
        tiles: tiles.concat(),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances,
    };
    Ok((section, shares.iter().map(|share| share.report).collect()))
}

/// Each animated light's reference value, by roster index: the runtime's
/// descriptors are the SH section's, one per AnimatedBakedLights row.
fn references(descriptors: &[AnimationDescriptor]) -> Vec<[f32; 3]> {
    descriptors.iter().map(reference_value).collect()
}

/// Every light the baked transport names must have a descriptor, and the
/// delta's descriptor table must be the identity: a share's roster index is
/// read straight into the runtime descriptor buffer (pin P10).
fn check_roster(inputs: &ReflectionProbeInputs<'_>, roster: usize) -> anyhow::Result<()> {
    if let Some(delta) = inputs.delta_sh {
        anyhow::ensure!(
            delta.animation_descriptor_indices.len() == roster
                && delta
                    .animation_descriptor_indices
                    .iter()
                    .enumerate()
                    .all(|(slot, &descriptor)| descriptor as usize == slot),
            "reflection probes need the identity animated-light roster in DeltaShVolumes"
        );
        anyhow::ensure!(
            delta.affinity_lights.iter().all(|&l| (l as usize) < roster),
            "DeltaShVolumes names an animated light past the {roster}-light roster"
        );
    }
    if let Some(maps) = inputs.animated_weight_maps {
        anyhow::ensure!(
            maps.texel_lights
                .iter()
                .all(|t| (t.light_index as usize) < roster),
            "AnimatedLightWeightMaps names an animated light past the {roster}-light roster"
        );
    }
    Ok(())
}

fn log_probes(section: &ReflectionProbesSection, reports: &[ShareReport]) {
    for (index, (probe, report)) in section.probes.iter().zip(reports).enumerate() {
        let faces = section
            .bindings
            .iter()
            .filter(|b| b.probe as usize == index)
            .count();
        log::info!(
            "[ReflectionProbes] probe {index}: entity {}, box {:?}..{:?}, capture point {:?}, \
             {faces} face(s)",
            probe.source_entity,
            probe.box_min,
            probe.box_max,
            probe.capture_point,
        );
        if report.animated_lights == 0 {
            log::info!("[ReflectionProbes] probe {index}: no animated light reaches its capture");
            continue;
        }
        let stored: Vec<String> = probe
            .lights
            .iter()
            .zip(report.totals)
            .filter(|(light, _)| light.is_used())
            .map(|(light, total)| {
                format!(
                    "light {} (reference [{:.3}, {:.3}, {:.3}], {:.3} of the capture)",
                    light.roster_index,
                    light.reference[0],
                    light.reference[1],
                    light.reference[2],
                    total
                )
            })
            .collect();
        log::info!(
            "[ReflectionProbes] probe {index}: covered share {:.3} of {} animated light(s); \
             stored {}",
            report.covered,
            report.animated_lights,
            stored.join(", "),
        );
        let held = report.animated_lights as usize - stored.len();
        if held > 0 {
            log::info!(
                "[ReflectionProbes] probe {index}: {held} animated light(s) past the \
                 {REFLECTION_PROBE_LIGHTS_PER_PROBE} stored stay at reference; covered share {:.3}",
                report.covered,
            );
        }
    }
    log::info!(
        "[ReflectionProbes] {} probe(s), {} binding(s), {} tile byte(s)",
        section.probes.len(),
        section.bindings.len(),
        section.tiles.len()
    );
}
