//! Reflection-probe capture stage (PRL id 54): place authored and derived
//! probes over up-facing fluid faces, capture each probe's static world into
//! an octahedral colour tile and a distance tile.
//! See: context/lib/resource_management.md §5 · build_pipeline.md §Compiler pipeline, §Build Cache

mod animated_direct;
mod authored;
mod box_fit;
mod capture;
mod capture_exclusion;
mod capture_fill;
mod capture_point;
mod capture_visibility;
mod delta_sample;
mod derived;
mod exclusion;
mod hit;
mod lightmap_sample;
mod materials;
pub(crate) mod placement;
mod reference;
mod sdf_lights;
mod sh_sample;
mod shading;
mod share;
mod stage_key;

#[cfg(test)]
mod capture_exclusion_test_fixtures;
#[cfg(test)]
mod capture_exclusion_tests;
#[cfg(test)]
mod capture_fill_tests;
#[cfg(test)]
mod capture_tests;
#[cfg(test)]
mod distance_tests;
#[cfg(test)]
mod exclusion_tests;
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
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
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
use crate::map_data::{BrushVolume, MapReflectionVolume};
use crate::partition::BspTree;

use animated_direct::AnimatedDirectSampler;
use box_fit::WorldBounds;
use capture::{RowCapture, TILE_INTERIOR, capture_row, encode_distance_tile, encode_tile};
use capture_exclusion::{CaptureBrushes, ProbeExclusion};
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
use share::{ShareReport, ShareSums, log_negative_references};
use stage_key::{cache_key, cache_payload, parse_cache_payload};

/// Cache stage identity for the whole id-54 section.
const STAGE_ID: &str = "reflection_probes";
/// Bump when capture shading, sampling or the payload changes. Placement
/// enters the key as its output, so a placement change re-keys by itself.
/// 3: animated direct and indirect at reference, sdf lights, light share.
/// 4: the distance tile (mean inverse L1 hit distance per texel).
/// 5, 6: unused; never reuse them, as their entries may sit in local caches.
/// 7: a finite hit never stores distance 0.
/// 8: shares over the upper hemisphere; derived box fit stops at the ceiling;
/// ties within tolerance; SH clamped after the deltas, as the runtime does;
/// the emissive mip from its own texel footprint; a light with a negative
/// reference never stored.
/// 9: a wall past a column's first ceiling hit still tops it when farther
/// out (a sunken pool under an overhanging lip fits the room); the key
/// gains id 45's descriptor table.
const STAGE_VERSION: u32 = 9;

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
    /// The static world's brush hulls (`MapData::brush_volumes`) and each
    /// post-cut face's brush, index-aligned with `geometry`'s faces. They
    /// reach the capture only through a probe's excluded set, so they key it
    /// through that set.
    pub brush_volumes: &'a [BrushVolume],
    pub face_brushes: &'a [Option<u32>],
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
    /// Id 45 as the level ships it. The capture reads none of it; the roster
    /// check refuses a descriptor table that is not the identity, as the
    /// loader does, so only that table enters the key.
    pub animated_direct_sh: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    /// Id 18: the runtime's lights, whose non-dynamic records are the spec
    /// lights the forward sdf loop shades.
    pub alpha_lights: &'a AlphaLightsSection,
    /// Id 23: the chunk windows the sdf selection reads.
    pub chunk_light_list: &'a ChunkLightListSection,
    /// Texture name → `.prm` key, from the TextureMips stage.
    pub texture_keys: &'a HashMap<String, [u8; 32]>,
    pub prm_root: &'a Path,
}

/// The bindings placement gives `environment` over a compiled static world,
/// as the stage places them: for tests comparing two face sets of one level.
#[cfg(test)]
pub(crate) fn placed_bindings(
    environment: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    world: &crate::fixture_pipeline::FixturePipeline,
) -> Vec<postretro_level_format::reflection_probes::ReflectionProbeBinding> {
    let bounds =
        WorldBounds::from_positions(world.geometry.geometry.vertices.iter().map(|v| &v.position))
            .expect("the level has static world");
    let tracer = SceneTracer::new(&world.bvh, &world.primitives, &world.geometry);
    let classifier = PointClassifier::new(&world.tree, &world.exterior_leaves, environment);
    let control = BakeControl::unrestricted();
    let probes = place_probes(
        environment,
        authored,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        MAX_REFLECTION_PROBES,
    )
    .expect("placement keeps within the cap");
    bindings(&probes)
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

    // References are a cheap function of the keyed descriptors; computed
    // before the lookup so a warm build logs the lights no probe stores, as
    // a cold one does.
    let references = references(&inputs.sh_volume.animation_descriptors);
    log_negative_references(&references);

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

    check_roster(inputs, references.len())?;
    let sh = ShVolumeSampler::new(inputs.sh_volume, PRODUCTION_PROBE_OCCLUSION)?;
    let deltas = inputs
        .delta_sh
        .map(|delta| {
            DeltaShSampler::new(
                delta,
                &sh,
                inputs.sh_volume.tile_dimension,
                inputs.sh_volume.grid_dimensions,
                &references,
            )
        })
        .transpose()?;
    let scene = CaptureScene {
        tracer,
        brushes: CaptureBrushes {
            volumes: inputs.brush_volumes,
            face_brushes: inputs.face_brushes,
        },
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
        max_distance: bounds.diagonal() as f32 + 1.0,
    };
    // No probe excludes a brush until per-probe classification feeds these
    // sets, so every capture is the unfiltered one.
    let excluded = vec![Vec::new(); probes.len()];
    let (section, reports) = capture_section(&scene, &probes, &excluded, &references, control)?;
    if let Some(cache) = stage_cache {
        cache.put(&key, &cache_payload(&section, &reports));
    }
    log_probes(&section, &reports);
    warn_hidden_water(&section, &probes, inputs.environment_volumes);
    Ok(Some(section))
}

/// Capture every probe's colour and distance tiles and light share, each
/// probe's primary rays skipping its `excluded` brushes (sorted brush
/// indices, one set per probe). Work items are tile rows, so one probe
/// spreads across workers; rows land in item order and each probe's share
/// sums merge in row order, so the bytes never depend on the worker count.
fn capture_section(
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

/// Every light the baked transport names must have a descriptor, and each
/// shipped descriptor table (ids 27 and 45) must be the identity over the
/// whole roster: a share's roster index indexes the runtime descriptor
/// buffer directly, so roster slot `r` must name descriptor `r`. This
/// refuses a table of another length as well as a non-identity entry. The
/// loader's check of section 54 (`validate_reflection_probe_lights` in
/// `postretro-level-loader`) is narrower: identity over each table's own
/// length, and every stored share's light inside the roster.
fn check_roster(inputs: &ReflectionProbeInputs<'_>, roster: usize) -> anyhow::Result<()> {
    let tables = [
        (
            "DeltaShVolumes (id 27)",
            inputs
                .delta_sh
                .map(|delta| delta.animation_descriptor_indices.as_slice()),
        ),
        (
            "AnimatedDirectShDeltaVolumes (id 45)",
            inputs
                .animated_direct_sh
                .map(|section| section.animation_descriptor_indices.as_slice()),
        ),
    ];
    for (table, indices) in tables {
        let Some(indices) = indices else {
            continue;
        };
        anyhow::ensure!(
            indices.len() == roster,
            "{table} has {} animated light(s) over the {roster}-light roster; reflection \
             probes need the identity roster",
            indices.len(),
        );
        if let Some((slot, descriptor)) = indices
            .iter()
            .enumerate()
            .find(|&(slot, &descriptor)| descriptor as usize != slot)
        {
            anyhow::bail!(
                "{table} maps animated light {slot} to descriptor {descriptor}; reflection \
                 probes need the identity roster"
            );
        }
    }
    if let Some(delta) = inputs.delta_sh {
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
